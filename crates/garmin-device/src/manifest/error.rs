use thiserror::Error;

use super::{DataType, ManifestFormat, TransferDirection};

/// A device-metadata document rejection.
#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("device metadata exceeds 4 MB")]
    TooLarge,
    #[error("malformed device metadata document: {0}")]
    MalformedDocument(String),
    #[error("invalid {format} document: {reason}")]
    InvalidDocument {
        /// Detected document format.
        format: ManifestFormat,
        /// Format-specific parser diagnostic.
        reason: String,
    },
    #[error("unsupported device metadata format: root={root:?} namespace={namespace:?}")]
    UnsupportedFormat {
        /// Local name of the root element.
        root: String,
        /// Resolved root namespace, or an empty string for an unbound root.
        namespace: String,
    },
    #[error("invalid location for {data_type}: {reason}")]
    InvalidLocation {
        /// Data-type name.
        data_type: String,
        /// Diagnostic reason.
        reason: &'static str,
    },
    #[error("unsupported transfer direction for {data_type}: {direction}")]
    UnsupportedDirection {
        /// Data-type name.
        data_type: String,
        /// Declared direction.
        direction: String,
    },
    #[error("ambiguous duplicate declaration for {data_type:?} {direction:?}")]
    AmbiguousCapability {
        /// Normalized manifest data type.
        data_type: DataType,
        /// Declared transfer direction.
        direction: TransferDirection,
    },
}
