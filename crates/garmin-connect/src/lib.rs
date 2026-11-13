//! Bounded, read-only Garmin Personal API client.
//! Secrets and response bodies never enter errors.
//! Async checks require an active Tokio runtime.

use garmin_model::{
    artifact::ByteCount,
    garmin_connect::{AccountFacts, PersonalApiKey},
};
use reqwest::{
    StatusCode,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use std::time::Duration;
use thiserror::Error;

include!(concat!(env!("OUT_DIR"), "/modules.rs"));
mod activities;
pub use activities::Download;

const ORIGIN: &str = "https://personal.connectapi.garmin.com";
const TOKEN_PATH: &str = "/personalapitoken-service/personal-api/personal-api-key/token";
const TOKEN_LIMIT: ByteCount = ByteCount::from_kibibytes(16);
const PROFILE_LIMIT: ByteCount = ByteCount::from_kibibytes(256);

/// A credential-free client configuration;
/// each check obtains a fresh access token.
pub struct Client {
    origin: String,
}

impl Default for Client {
    fn default() -> Self {
        Self {
            origin: ORIGIN.into(),
        }
    }
}

impl Client {
    /// Exchanges the key and reads typed profile facts from the official service.
    /// # Errors
    /// Reports bounded, redacted transport, authorization, or response failures.
    pub async fn check(&self, key: &PersonalApiKey) -> Result<AccountFacts, Error> {
        Ok(self.authenticate(key).await?.0)
    }

    async fn authenticate(
        &self,
        key: &PersonalApiKey,
    ) -> Result<(AccountFacts, api::HttpClient), Error> {
        let transport = http(HeaderMap::new())?;
        let mut authorization = HeaderValue::from_str(&format!("ApiKey {}", key.expose()))
            .map_err(|_| Error::InvalidToken)?;
        authorization.set_sensitive(true);
        let response = transport
            .post(format!("{}{TOKEN_PATH}", self.origin))
            .header(AUTHORIZATION, authorization)
            .send()
            .await
            .map_err(network)?;
        status(response.status(), "key exchange")?;
        let token = body(response, limit_bytes(TOKEN_LIMIT)?).await?;
        let token = std::str::from_utf8(&token)
            .map_err(|_| Error::InvalidToken)?
            .trim();
        if token.is_empty() || !token.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(Error::InvalidToken);
        }
        let mut bearer =
            HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| Error::InvalidToken)?;
        bearer.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, bearer);
        let client = api::HttpClient::with_client(http(headers)?)
            .with_base_url(&self.origin)
            .with_max_response_body_bytes(limit_bytes(PROFILE_LIMIT)?);
        let profile = client
            .get_userprofile_service_userprofile_settings()
            .await
            .map_err(profile_error)?;
        let facts = AccountFacts {
            display_name: account_identity(profile.display_name)?,
            locale: fact(profile.preferred_locale),
            measurement_system: fact(profile.measurement_system),
            time_zone: fact(profile.time_zone),
        };
        if facts.display_name.is_none()
            && facts.locale.is_none()
            && facts.measurement_system.is_none()
            && facts.time_zone.is_none()
        {
            return Err(Error::EmptyProfile);
        }
        Ok((facts, client))
    }
}

fn account_identity(value: Option<String>) -> Result<Option<String>, Error> {
    let value = value.filter(|value| !value.trim().is_empty());
    // Identity must stay exact: truncation would merge distinct accounts.
    if value
        .as_ref()
        .is_some_and(|value| value.chars().count() > 256)
    {
        return Err(Error::InvalidProfile);
    }
    Ok(value)
}

fn fact(value: Option<String>) -> Option<String> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.chars().take(256).collect())
}

fn http(headers: HeaderMap) -> Result<reqwest::Client, Error> {
    let builder = reqwest::Client::builder()
        .default_headers(headers)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15));
    #[cfg(test)]
    let builder = builder.no_proxy();
    builder.build().map_err(network)
}

fn network(error: reqwest::Error) -> Error {
    if error.without_url().is_timeout() {
        Error::Timeout
    } else {
        Error::Network
    }
}

fn status(status: StatusCode, operation: &'static str) -> Result<(), Error> {
    if status.is_success() {
        Ok(())
    } else {
        Err(Error::Rejected {
            operation,
            status: status.as_u16(),
        })
    }
}

fn limit_bytes(limit: ByteCount) -> Result<usize, Error> {
    usize::try_from(limit.into_u64()).map_err(|_| Error::TooLarge)
}

async fn body(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, Error> {
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(Error::TooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network)? {
        if chunk.len() > limit - bytes.len() {
            return Err(Error::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn profile_error(error: api::ApiOpError<serde_json::Value>) -> Error {
    operation_error(error, "profile request")
}

fn operation_error(error: api::ApiOpError<serde_json::Value>, operation: &'static str) -> Error {
    match error {
        api::ApiOpError::Api(error) if error.status == 200 => Error::InvalidProfile,
        api::ApiOpError::Api(error) => Error::Rejected {
            operation,
            status: error.status,
        },
        api::ApiOpError::Transport(api::HttpError::ResponseTooLarge { .. }) => Error::TooLarge,
        api::ApiOpError::Transport(api::HttpError::Network(error)) => network(error),
        api::ApiOpError::Transport(_) => Error::Network,
    }
}

/// Safe diagnostics: no credential, URL, response body,
/// or underlying HTTP error is retained.
#[derive(Debug, Error)]
pub enum Error {
    #[error("Could not reach Garmin Connect")]
    Network,
    #[error("Garmin Connect did not respond in time")]
    Timeout,
    #[error("Garmin rejected the {operation} (HTTP {status})")]
    Rejected {
        operation: &'static str,
        status: u16,
    },
    #[error("Garmin returned an invalid access token")]
    InvalidToken,
    #[error("Garmin returned an invalid profile response")]
    InvalidProfile,
    #[error("Garmin returned no profile facts")]
    EmptyProfile,
    #[error("Garmin did not return an account identifier")]
    MissingAccountIdentity,
    #[error("Garmin returned invalid activity data")]
    InvalidActivities,
    #[error("Activity ID or page offset is invalid")]
    InvalidSelection,
    #[error("Garmin response exceeded its size limit")]
    TooLarge,
}

#[cfg(test)]
mod tests;
