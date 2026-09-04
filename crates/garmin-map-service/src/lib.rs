//! Anonymous client for Garmin's official OMT map service.

mod client;
mod download;
mod model;

pub use client::{
    ClientIdentity, GARMIN_EXPRESS_USER_AGENT, OmtClient, OmtError, is_http_loopback_url,
    omt_update_endpoint,
};
pub use download::{DownloadAuthorizationError, authorize_download_url};
