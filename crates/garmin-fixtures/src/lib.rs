//! Publishable data for mock deployments and tests.

#[path = "../../../infra/fixtures/firmware/fixtures.rs"]
pub mod firmware;

#[cfg(feature = "demo")]
pub mod device;
#[cfg(feature = "demo")]
mod routes;
#[cfg(feature = "demo")]
mod seed;
#[cfg(feature = "demo")]
pub use seed::{SeedError, SeededActivity, SeededCorpus, recreate, seed};
