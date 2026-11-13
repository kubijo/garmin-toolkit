//! Profile-scoped credentials and activity browsing for the official Personal API.

use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use thiserror::Error;

/// Reusable owner-created key. Debug output never includes its contents.
#[derive(Clone, Serialize)]
#[serde(transparent)]
pub struct PersonalApiKey(String);

/// Non-secret facts returned by an authenticated profile request.
#[garmin_macros::portable(eq)]
pub struct AccountFacts {
    pub display_name: Option<String>,
    pub locale: Option<String>,
    pub measurement_system: Option<String>,
    pub time_zone: Option<String>,
}

/// User-authored cloud metadata shown alongside the original FIT preview.
#[garmin_macros::portable(eq)]
pub struct ActivityDetails {
    pub name: Option<String>,
    pub description: Option<String>,
}

/// One selectable activity from the official service, before FIT import.
#[garmin_macros::portable]
pub struct RemoteActivity {
    pub id: i64,
    pub imported: bool,
    pub sport: Option<crate::activity::ActivitySport>,
    pub name: String,
    pub started_at: Option<String>,
    pub distance_metres: Option<f64>,
    pub duration_seconds: Option<f64>,
}

/// Whether a failure concerns one activity or the shared connection.
#[garmin_macros::portable(eq)]
pub enum ActivityFailureKind {
    Activity,
    Session,
}

/// A redacted failure; session failures stop a sequence of activity requests.
#[garmin_macros::portable(eq)]
pub struct ActivityFailure {
    pub message: String,
    pub kind: ActivityFailureKind,
}

impl ActivityFailure {
    #[must_use]
    pub const fn stops_batch(&self) -> bool {
        matches!(self.kind, ActivityFailureKind::Session)
    }
}

/// Explicit, profile-scoped cloud activity operations.
#[garmin_macros::portable(eq)]
pub enum ActivityRequest {
    List { start: u32 },
    Preview { id: i64 },
    Prepare { id: i64 },
    Import { id: i64, digest: String },
}

/// One bounded page or one completed import.
#[garmin_macros::portable]
pub enum ActivityResponse {
    Page {
        start: u32,
        activities: Vec<RemoteActivity>,
        has_more: bool,
    },
    Imported {
        id: i64,
        activities: u64,
        duplicate: bool,
    },
}

impl ActivityResponse {
    /// Keeps the current browser page when an explicit import completes.
    pub fn merge(&mut self, response: Self) {
        if let (Self::Page { activities, .. }, Self::Imported { id, .. }) = (&mut *self, &response)
        {
            for activity in activities.iter_mut().filter(|activity| activity.id == *id) {
                activity.imported = true;
            }
        } else {
            *self = response;
        }
    }
}

impl PersonalApiKey {
    /// Validates a key without contacting Garmin.
    /// # Errors
    /// Rejects empty, oversized, or non-token text.
    pub fn from_string(value: String) -> Result<Self, InvalidKey> {
        let trimmed = value.trim();
        if trimmed.is_empty()
            || trimmed.len() > 8192
            || !trimmed.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(InvalidKey);
        }
        if trimmed.len() == value.len() {
            Ok(Self(value))
        } else {
            Ok(Self(trimmed.to_owned()))
        }
    }

    /// Exposes the credential only at persistence and authentication boundaries.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PersonalApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PersonalApiKey([redacted])")
    }
}

impl<'de> Deserialize<'de> for PersonalApiKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_string(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Invalid credential text; never carries the rejected input.
#[derive(Clone, Copy, Debug, Error)]
#[error("Enter a Personal API key without spaces or line breaks")]
pub struct InvalidKey;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_receipt_preserves_page_and_marks_only_the_matching_row() {
        let mut page = ActivityResponse::Page {
            start: 20,
            activities: [42, 43]
                .map(|id| RemoteActivity {
                    id,
                    imported: false,
                    sport: None,
                    name: id.to_string(),
                    started_at: None,
                    distance_metres: None,
                    duration_seconds: None,
                })
                .to_vec(),
            has_more: true,
        };
        for duplicate in [false, true] {
            page.merge(ActivityResponse::Imported {
                id: 42,
                activities: 1,
                duplicate,
            });
            let ActivityResponse::Page {
                start,
                activities,
                has_more,
            } = &page
            else {
                panic!("lost browser page")
            };
            assert_eq!(*start, 20);
            assert!(*has_more);
            assert!(activities[0].imported);
            assert!(!activities[1].imported);
        }
    }

    #[test]
    fn debug_and_validation_do_not_disclose_keys() {
        let key = PersonalApiKey::from_string(" secret-test-key ".into()).unwrap();
        assert_eq!(key.expose(), "secret-test-key");
        assert!(!format!("{key:?}").contains("secret-test-key"));
        assert!(PersonalApiKey::from_string("bad\nkey".into()).is_err());
        let invalid = postcard::to_allocvec("bad key").unwrap();
        assert!(postcard::from_bytes::<PersonalApiKey>(&invalid).is_err());
    }
}
