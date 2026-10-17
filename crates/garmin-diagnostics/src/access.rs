use axum::{
    extract::{ConnectInfo, Request},
    http::{HeaderMap, StatusCode, header, uri::Authority},
    middleware::Next,
    response::{IntoResponse as _, Response},
};
use std::net::SocketAddr;

fn local_host(headers: &HeaderMap) -> bool {
    headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok()?.parse::<Authority>().ok())
        .is_some_and(|authority| {
            let host = authority.host().trim_matches(['[', ']']);
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.to_canonical().is_loopback())
        })
}

pub(crate) async fn local_only(request: Request, next: Next) -> Response {
    let headers = request.headers();
    let peer = request.extensions().get::<ConnectInfo<SocketAddr>>();
    if !peer.is_some_and(|peer| peer.0.ip().to_canonical().is_loopback())
        || !local_host(headers)
        || headers.keys().any(|key| {
            key == "forwarded" || key == "x-real-ip" || key.as_str().starts_with("x-forwarded-")
        })
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !same_origin(&request)
        || headers
            .get("sec-fetch-site")
            .is_some_and(|value| value != "same-origin" && value != "none")
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = next.run(request).await;
    for (name, value) in [
        (header::CACHE_CONTROL, "no-store"),
        (header::VARY, "Accept"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; script-src 'self'; connect-src 'self'; style-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'",
        ),
    ] {
        response
            .headers_mut()
            .insert(name, axum::http::HeaderValue::from_static(value));
    }
    response
}

fn same_origin(request: &Request) -> bool {
    let headers = request.headers();
    headers.get(header::ORIGIN).is_none_or(|origin| {
        origin
            .to_str()
            .ok()
            .and_then(|origin| url::Url::parse(origin).ok())
            .is_some_and(|origin| {
                let host = headers
                    .get(header::HOST)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default();
                let expected = format!("{}://{host}", request.uri().scheme_str().unwrap_or("http"));
                url::Url::parse(&expected)
                    .is_ok_and(|expected| origin.origin() == expected.origin())
            })
    })
}
