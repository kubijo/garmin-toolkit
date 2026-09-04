use crate::model::wire;
use bytes::BytesMut;
use futures_util::StreamExt;
use garmin_capture::{CaptureError, HttpHeader, HttpRequestHead, HttpResponseHead, SessionCapture};
use garmin_model::map::{InstalledMapFile, MapAuthorization, MapCatalog, MapInstallIdentifier};
use reqwest::header::{
    ACCEPT, ACCEPT_LANGUAGE, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue,
};
use reqwest::{Client, StatusCode};
use std::time::Duration;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use url::Url;
use uuid::Uuid;

const OMT_BASE: &str = "https://omt.garmin.com/";
const OMT_UPDATE_PATH: &str = "api/maps/universal/update";
const MAX_API_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[must_use]
/// Return the production map-update endpoint.
/// # Panics
/// Panics if either source-controlled URL component is invalid.
pub fn omt_update_endpoint() -> Url {
    Url::parse(OMT_BASE)
        .and_then(|base| base.join(OMT_UPDATE_PATH))
        .expect("the built-in Garmin map-update endpoint must be a valid URL")
}

/// Compatibility user agent sent to Garmin's map and download services.
pub const GARMIN_EXPRESS_USER_AGENT: &str = "RestSharp/112.0.0.0";

#[derive(Debug, Clone)]
pub struct ClientIdentity {
    pub name: String,
    pub version: String,
    pub platform: String,
    pub platform_version: String,
    pub locale: String,
}

impl Default for ClientIdentity {
    fn default() -> Self {
        Self {
            name: "EXPRESS".to_owned(),
            version: "7.29.1.0".to_owned(),
            platform: "WINDOWS".to_owned(),
            platform_version: "10.0".to_owned(),
            locale: "en-US".to_owned(),
        }
    }
}

#[derive(Clone)]
pub struct OmtClient {
    http: Client,
    base: Url,
    capture: Option<SessionCapture>,
}

impl OmtClient {
    /// Create a client for Garmin's production OMT service without credentials.
    /// # Errors
    /// [`OmtError`] for invalid service, identity, or HTTP configuration.
    pub fn anonymous(identity: &ClientIdentity) -> Result<Self, OmtError> {
        Self::with_base(identity, Url::parse(OMT_BASE)?, false)
    }

    /// Create a client for a loopback-only mock OMT service.
    /// # Errors
    /// [`OmtError`] if the URL does not use HTTP with a literal loopback
    /// address, or if headers and the HTTP client cannot be constructed.
    pub fn local_mock(identity: &ClientIdentity, base: Url) -> Result<Self, OmtError> {
        if !is_http_loopback_url(&base) {
            return Err(OmtError::NonLoopbackMock(base));
        }
        Self::with_base(identity, base, true)
    }

    fn with_base(
        identity: &ClientIdentity,
        base: Url,
        loopback_http: bool,
    ) -> Result<Self, OmtError> {
        let mut headers = HeaderMap::new();
        insert(&mut headers, "Garmin-Client-Name", &identity.name)?;
        insert(&mut headers, "Garmin-Client-Version", &identity.version)?;
        insert(&mut headers, "Garmin-Client-Platform", &identity.platform)?;
        insert(
            &mut headers,
            "Garmin-Client-Platform-Version",
            &identity.platform_version,
        )?;
        insert(&mut headers, "Garmin-Client-LocaleCode", &identity.locale)?;
        insert(
            &mut headers,
            "Garmin-Client-SessionId",
            &Uuid::new_v4().to_string(),
        )?;
        headers.insert(
            ACCEPT_LANGUAGE,
            HeaderValue::from_str(&identity.locale).map_err(OmtError::InvalidHeader)?,
        );
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let mut builder = Client::builder()
            .default_headers(headers)
            .user_agent(GARMIN_EXPRESS_USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none());
        if loopback_http {
            builder = builder.tls_certs_only(std::iter::empty::<reqwest::Certificate>());
        }
        let http = builder.build()?;
        Ok(Self {
            http,
            base,
            capture: None,
        })
    }

    #[must_use]
    pub fn with_capture(mut self, capture: Option<SessionCapture>) -> Self {
        self.capture = capture;
        self
    }

    /// Request available universal-map updates using the complete device XML.
    /// # Errors
    /// [`OmtError`] for transport, status, or response-schema failures.
    pub async fn check_maps(
        &self,
        garmin_device_xml: &str,
        installed_map_files: &[InstalledMapFile],
    ) -> Result<MapCatalog, OmtError> {
        let body = wire::GetUpdatesRequest {
            garmin_device_xml,
            options: wire::UpdateOptions::default(),
        };
        let response: wire::GetUpdatesResponse = self.post(OMT_UPDATE_PATH, &body).await?;
        let mut catalog: MapCatalog = response.into();
        catalog.attach_installed_versions(installed_map_files);
        Ok(catalog)
    }

    /// Obtain map unlock material before changing the device.
    /// # Errors
    /// [`OmtError`] for transport, status, or response-schema failures.
    pub async fn activate(
        &self,
        garmin_device_xml: &str,
        maps: &[MapInstallIdentifier],
    ) -> Result<MapAuthorization, OmtError> {
        let body = wire::ActivateUpdatesRequest {
            garmin_device_xml,
            installed_maps: maps.iter().map(Into::into).collect(),
        };
        let response: wire::ActivateUpdatesResponse =
            self.post("api/maps/universal/activate", &body).await?;
        Ok(response.into())
    }

    async fn post<T: serde::Serialize + ?Sized, R: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<R, OmtError> {
        let url = self.base.join(path)?;
        let body = serde_json::to_vec(body).map_err(OmtError::RequestJson)?;
        let request = self
            .http
            .post(url)
            .header(CONTENT_LENGTH, body.len())
            .body(body.clone())
            .build()?;
        let exchange = match &self.capture {
            Some(capture) => Some(
                capture
                    .begin_http(
                        if path.ends_with("/activate") {
                            "omt-activate"
                        } else {
                            "omt-update"
                        },
                        &HttpRequestHead {
                            method: request.method().as_str(),
                            url: request.url().as_str(),
                            version: &format!("{:?}", request.version()),
                            headers: capture_headers(request.headers()),
                        },
                        &body,
                    )
                    .await?,
            ),
            None => None,
        };
        let response = match self.http.execute(request).await {
            Ok(response) => response,
            Err(error) => {
                if let Some(exchange) = &exchange {
                    exchange.error(&error.to_string()).await?;
                }
                return Err(error.into());
            }
        };
        let status = response.status();
        if let Some(exchange) = &exchange {
            exchange
                .response_head(&HttpResponseHead {
                    status: status.as_u16(),
                    status_text: status.canonical_reason().unwrap_or_default(),
                    version: &format!("{:?}", response.version()),
                    headers: capture_headers(response.headers()),
                })
                .await?;
        }
        let bytes = match bounded_response_body(response, exchange.as_ref()).await {
            Ok(bytes) => bytes,
            Err(error) => {
                if let Some(exchange) = &exchange {
                    exchange.error(&error.to_string()).await?;
                }
                return Err(error);
            }
        };
        if !status.is_success() {
            return Err(OmtError::HttpStatus {
                status,
                summary: sanitize_server_error(&bytes),
            });
        }
        serde_json::from_slice(&bytes).map_err(OmtError::Json)
    }
}

fn capture_headers(headers: &HeaderMap) -> Vec<HttpHeader> {
    headers
        .iter()
        .map(|(name, value)| HttpHeader {
            name: name.as_str().to_owned(),
            value_bytes: value.as_bytes().to_vec(),
        })
        .collect()
}

#[must_use]
pub fn is_http_loopback_url(url: &Url) -> bool {
    if url.scheme() != "http" {
        return false;
    }
    match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(_)) | None => false,
    }
}

async fn bounded_response_body(
    response: reqwest::Response,
    exchange: Option<&garmin_capture::HttpExchange>,
) -> Result<BytesMut, OmtError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_API_RESPONSE_BYTES as u64)
    {
        return Err(OmtError::ResponseTooLarge);
    }
    let mut body = BytesMut::new();
    let mut captured = match exchange {
        Some(exchange) => Some(exchange.response_body_file().await?),
        None => None,
    };
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if let Some(captured) = &mut captured {
            captured
                .write_all(&chunk)
                .await
                .map_err(CaptureError::from)?;
        }
        if body.len().saturating_add(chunk.len()) > MAX_API_RESPONSE_BYTES {
            if let Some(captured) = &mut captured {
                captured.flush().await.map_err(CaptureError::from)?;
                captured.sync_all().await.map_err(CaptureError::from)?;
            }
            return Err(OmtError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    if let Some(mut captured) = captured {
        captured.flush().await.map_err(CaptureError::from)?;
        captured.sync_all().await.map_err(CaptureError::from)?;
    }
    if let Some(exchange) = exchange {
        exchange.complete_response_body(body.len()).await?;
    }
    Ok(body)
}

fn insert(headers: &mut HeaderMap, name: &'static str, value: &str) -> Result<(), OmtError> {
    headers.insert(
        name,
        HeaderValue::from_str(value).map_err(OmtError::InvalidHeader)?,
    );
    Ok(())
}

fn sanitize_server_error(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(240).collect()
}

#[derive(Debug, Error)]
pub enum OmtError {
    #[error("invalid service URL: {0}")]
    Url(#[from] url::ParseError),
    #[error("mock service URL must use HTTP on loopback: {0}")]
    NonLoopbackMock(Url),
    #[error("invalid client header: {0}")]
    InvalidHeader(reqwest::header::InvalidHeaderValue),
    #[error("Garmin service request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Garmin service returned {status}: {summary}")]
    HttpStatus { status: StatusCode, summary: String },
    #[error("Garmin's map-update response could not be parsed: {0}")]
    Json(serde_json::Error),
    #[error("Garmin service request could not be serialized: {0}")]
    RequestJson(serde_json::Error),
    #[error("session capture failed: {0}")]
    Capture(#[from] CaptureError),
    #[error("Garmin service response exceeded 16 MB")]
    ResponseTooLarge,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::response::Redirect;
    use axum::routing::post;
    use garmin_device::{TransportKind, parse_manifest};

    #[test]
    fn mock_clients_are_restricted_to_loopback() {
        let identity = ClientIdentity::default();
        assert!(
            OmtClient::local_mock(&identity, Url::parse("http://127.0.0.1:39765/").unwrap())
                .is_ok()
        );
        assert!(
            OmtClient::local_mock(&identity, Url::parse("http://localhost:39765/").unwrap())
                .is_err()
        );
        assert!(
            OmtClient::local_mock(&identity, Url::parse("https://example.com/").unwrap()).is_err()
        );
    }

    #[tokio::test]
    async fn metadata_requests_do_not_follow_redirects() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new().route(
            "/api/maps/universal/update",
            post(|| async { Redirect::temporary("http://127.0.0.1:9/capture") }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, router).await });
        let client = OmtClient::local_mock(
            &ClientIdentity::default(),
            Url::parse(&format!("http://{address}/")).unwrap(),
        )
        .unwrap();
        let manifest = parse_manifest(
            indoc::indoc! {r#"
                <Device xmlns="http://www.garmin.com/xmlschemas/GarminDevice/v2">
                  <Model>
                    <SoftwareVersion>100</SoftwareVersion>
                    <Description>Test</Description>
                  </Model>
                  <Id>42</Id>
                  <MassStorageMode />
                </Device>
            "#},
            TransportKind::MassStorage,
            "test".to_owned(),
        )
        .unwrap();

        let result = client
            .check_maps(
                manifest.raw_xml(),
                manifest.capabilities().installed_map_files(),
            )
            .await;

        assert!(matches!(
            result,
            Err(OmtError::HttpStatus {
                status: StatusCode::TEMPORARY_REDIRECT,
                ..
            })
        ));
        server.abort();
    }
}
