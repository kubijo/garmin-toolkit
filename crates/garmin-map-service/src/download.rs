use md5::{Digest as _, Md5};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use url::Url;

const TOKEN_LIFETIME_SECONDS: u64 = 24 * 60 * 60;
const TOKEN_PARAMETER: &str = "garmindlm";
const TOKEN_SALT: &[u8] = b"XQVM5coEWp8QJ";

/// Add Garmin's request token to a protected map-download URL.
/// # Errors
/// The host clock is invalid or the expiry exceeds Garmin's timestamp range.
pub fn authorize_download_url(source: &Url) -> Result<Url, DownloadAuthorizationError> {
    if source
        .query_pairs()
        .any(|(name, _)| name == TOKEN_PARAMETER)
    {
        return Ok(source.clone());
    }
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs()
        .checked_add(TOKEN_LIFETIME_SECONDS)
        .ok_or(DownloadAuthorizationError::ExpiryOverflow)?;
    let expires = u32::try_from(expires).map_err(|_| DownloadAuthorizationError::ExpiryOverflow)?;
    Ok(tokenized_url(source, expires))
}

fn tokenized_url(source: &Url, expires: u32) -> Url {
    let relative = source.query().map_or_else(
        || source.path().to_owned(),
        |query| format!("{}?{query}", source.path()),
    );
    let mut first = Md5::new();
    first.update(expires.to_le_bytes());
    first.update(relative.as_bytes());
    first.update(TOKEN_SALT);

    let mut second = Md5::new();
    second.update(TOKEN_SALT);
    second.update(first.finalize());
    let token = format!("{expires}_{}", hex::encode(second.finalize()));

    let mut authorized = source.clone();
    authorized
        .query_pairs_mut()
        .append_pair(TOKEN_PARAMETER, &token);
    authorized
}

#[derive(Debug, Error)]
pub enum DownloadAuthorizationError {
    #[error("system clock is before the Unix epoch: {0}")]
    Clock(#[from] std::time::SystemTimeError),
    #[error("download-token expiry exceeds the supported timestamp range")]
    ExpiryOverflow,
}

#[cfg(test)]
mod tests {
    use super::{TOKEN_PARAMETER, authorize_download_url, tokenized_url};

    #[test]
    fn token_covers_path_and_existing_query() {
        let source = url::Url::parse("https://example.invalid/maps/base.img?part=2").unwrap();
        let authorized = tokenized_url(&source, 1_800_000_000);

        assert_eq!(authorized.scheme(), "https");
        assert_eq!(authorized.host_str(), Some("example.invalid"));
        assert_eq!(authorized.path(), "/maps/base.img");
        assert_eq!(
            authorized
                .query_pairs()
                .filter(|(name, _)| name == TOKEN_PARAMETER)
                .count(),
            1
        );
        assert!(
            authorized.query_pairs().any(|(name, value)| {
                name == TOKEN_PARAMETER && value.starts_with("1800000000_")
            })
        );
    }

    #[test]
    fn existing_token_is_not_replaced() {
        let source = url::Url::parse("https://example.invalid/map.img?garmindlm=existing").unwrap();

        assert_eq!(authorize_download_url(&source).unwrap(), source);
    }
}
