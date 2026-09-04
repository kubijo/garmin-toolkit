//! Source-neutral application domain.

macro_rules! define_id {
    ($kind:ident, $name:ident, $tag:literal, $kind_doc:literal, $name_doc:literal) => {
        #[doc = $kind_doc]
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum $kind {}

        impl newtype_uuid::TypedUuidKind for $kind {
            fn tag() -> newtype_uuid::TypedUuidTag {
                const TAG: newtype_uuid::TypedUuidTag = newtype_uuid::TypedUuidTag::new($tag);
                TAG
            }
        }

        #[doc = $name_doc]
        pub type $name = newtype_uuid::TypedUuid<$kind>;
    };
}

macro_rules! text_value {
    ($name:ident, $error_type:ty, $error:expr, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            /// Parses trimmed, non-empty text.
            /// # Errors
            /// An error when the text is blank.
            pub fn from_string(value: String) -> Result<Self, $error_type> {
                let value = $crate::value::trimmed_owned(value);
                if value.is_empty() {
                    return Err($error);
                }
                Ok(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            #[must_use]
            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl std::str::FromStr for $name {
            type Err = $error_type;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::from_string(value.to_owned())
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

pub mod activity;
mod activity_fingerprint;
pub mod artifact;
pub mod device;
pub mod identity;
pub mod map;
pub mod observation;
pub mod route;
pub mod value;
