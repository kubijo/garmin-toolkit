//! Software updates offered for a connected device.
use crate::artifact::ByteCount;

#[garmin_macros::portable(eq)]
pub struct Component {
    pub part_number: String,
    pub semantic_version: String,
    pub name: String,
    pub description: Option<String>,
    pub installed: Option<String>,
    pub available: String,
    pub system_bundle: bool,
    pub release_notes: Vec<ReleaseNote>,
    pub download_bytes: Option<ByteCount>,
    pub staging_issue: Option<StagingIssue>,
    pub needs_restart: bool,
    pub has_metadata: bool,
}

#[garmin_macros::portable(eq)]
pub enum StagingIssue {
    InstallationInstructions,
    DeliveryRestriction(String),
}

impl std::fmt::Display for StagingIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InstallationInstructions => {
                f.write_str("Additional installation instructions are unsupported")
            }
            Self::DeliveryRestriction(name) => {
                write!(f, "Unsupported delivery restriction: {name}")
            }
        }
    }
}

#[garmin_macros::portable(eq)]
pub struct ReleaseNote {
    pub version: String,
    pub changes: Vec<String>,
}
