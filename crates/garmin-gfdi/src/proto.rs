//! Buffa-generated proto2 `FileAccess` types. Schemas do not authorize device mutations.

// Third-party generated names and documentation are outside handwritten lint policy.
#[expect(
    clippy::allow_attributes,
    clippy::allow_attributes_without_reason,
    reason = "Buffa emits its own lint attributes in third-party generated code"
)]
#[allow(
    clippy::all,
    clippy::pedantic,
    missing_docs,
    non_snake_case,
    reason = "Generated schema code retains upstream names and documentation"
)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/_generated.rs"));
}

pub use generated::GDI::Proto::{DataTypes as data_types, FileAccess as file_access};
pub use generated::garmin_vivomovehr::Smart;
