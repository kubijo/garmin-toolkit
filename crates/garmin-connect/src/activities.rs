use garmin_model::{
    artifact::ByteCount,
    garmin_connect::{AccountFacts, ActivityDetails, PersonalApiKey, RemoteActivity},
};

use crate::{Client, Error, fact, limit_bytes, operation_error};

const LIST_LIMIT: ByteCount = ByteCount::from_mebibytes(2);
const DETAILS_LIMIT: ByteCount = ByteCount::from_mebibytes(2);
const DOWNLOAD_LIMIT: ByteCount = ByteCount::from_mebibytes(32);
const DESCRIPTION_LIMIT: ByteCount = ByteCount::from_kibibytes(16);
pub const PAGE_SIZE: usize = 20;

/// Authenticated account identity and an untrusted original-file archive.
pub struct Download {
    pub account: AccountFacts,
    pub archive: Vec<u8>,
}

impl Client {
    /// Fetches one page, newest first, without importing or modifying cloud data.
    /// # Errors
    /// Rejects invalid pagination, service failures and malformed summaries.
    pub async fn activities(
        &self,
        key: &PersonalApiKey,
        start: u32,
    ) -> Result<(AccountFacts, Vec<RemoteActivity>), Error> {
        let start = i32::try_from(start).map_err(|_| Error::InvalidSelection)?;
        let (account, client) = self.authenticate(key).await?;
        let client = client.with_max_response_body_bytes(limit_bytes(LIST_LIMIT)?);
        let activities = client
            .get_activitylist_service_activities_search_activities_builder()
            .start(start)
            .limit(20)
            .sort_order("desc")
            .sort_by("startLocal")
            .send()
            .await
            .map_err(
                |error| match operation_error(error, "activity list request") {
                    Error::InvalidProfile => Error::InvalidActivities,
                    error => error,
                },
            )?;
        if activities.len() > PAGE_SIZE {
            return Err(Error::InvalidActivities);
        }
        let mut ids = std::collections::HashSet::with_capacity(activities.len());
        let activities = activities
            .into_iter()
            .map(|activity| {
                let id = activity
                    .activity_id
                    .filter(|id| *id > 0)
                    .ok_or(Error::InvalidActivities)?;
                if !ids.insert(id) {
                    return Err(Error::InvalidActivities);
                }
                Ok(RemoteActivity {
                    id,
                    imported: false,
                    sport: activity
                        .activity_type
                        .and_then(|kind| kind.type_key)
                        .and_then(|key| sport(&key)),
                    name: fact(activity.activity_name).unwrap_or_else(|| id.to_string()),
                    started_at: fact(activity.start_time_gmt),
                    distance_metres: activity
                        .distance
                        .filter(|value| value.is_finite() && *value >= 0.0),
                    duration_seconds: activity
                        .duration
                        .filter(|value| value.is_finite() && *value >= 0.0),
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok((account, activities))
    }

    /// Downloads an original archive for one positive activity ID.
    /// # Errors
    /// Rejects invalid IDs, missing account identity, and bounded transport failures.
    pub async fn download(&self, key: &PersonalApiKey, id: i64) -> Result<Download, Error> {
        if id <= 0 {
            return Err(Error::InvalidSelection);
        }
        let (account, client) = self.authenticate(key).await?;
        if account.display_name.is_none() {
            return Err(Error::MissingAccountIdentity);
        }
        original(account, client, id).await
    }

    /// Reads activity metadata and original FIT using the same authenticated account.
    /// # Errors
    /// Rejects invalid IDs, mismatched metadata and bounded transport failures.
    pub async fn preview(
        &self,
        key: &PersonalApiKey,
        id: i64,
    ) -> Result<(Download, ActivityDetails), Error> {
        if id <= 0 {
            return Err(Error::InvalidSelection);
        }
        let (account, client) = self.authenticate(key).await?;
        if account.display_name.is_none() {
            return Err(Error::MissingAccountIdentity);
        }
        let details = client
            .clone()
            .with_max_response_body_bytes(limit_bytes(DETAILS_LIMIT)?)
            .get_activity_service_activity_activity_id(id)
            .await
            .map_err(|error| match operation_error(error, "activity metadata") {
                Error::InvalidProfile => Error::InvalidActivities,
                error => error,
            })?;
        let description_limit = limit_bytes(DESCRIPTION_LIMIT)?;
        if details.activity_id != Some(id)
            || details
                .description
                .as_ref()
                .is_some_and(|text| text.len() > description_limit)
        {
            return Err(Error::InvalidActivities);
        }
        let details = ActivityDetails {
            name: fact(details.activity_name),
            description: details.description.filter(|text| !text.trim().is_empty()),
        };
        Ok((original(account, client, id).await?, details))
    }
}

async fn original(
    account: AccountFacts,
    client: crate::api::HttpClient,
    id: i64,
) -> Result<Download, Error> {
    let archive = client
        .with_max_response_body_bytes(limit_bytes(DOWNLOAD_LIMIT)?)
        .get_download_service_files_activity_activity_id(id)
        .await
        .map_err(|error| operation_error(error, "activity download"))?;
    Ok(Download {
        account,
        archive: archive.to_vec(),
    })
}

fn sport(key: &str) -> Option<garmin_model::activity::ActivitySport> {
    use garmin_model::activity::ActivitySport;
    if key.contains("cycling") || key.contains("biking") {
        Some(ActivitySport::Cycling)
    } else if key.contains("running") {
        Some(ActivitySport::Running)
    } else if key.contains("swimming") {
        Some(ActivitySport::Swimming)
    } else if key.contains("walking") {
        Some(ActivitySport::Walking)
    } else if key.contains("hiking") {
        Some(ActivitySport::Hiking)
    } else {
        None
    }
}
