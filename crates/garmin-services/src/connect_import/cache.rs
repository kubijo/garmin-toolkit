use garmin_model::{
    garmin_connect::{
        AccountFacts, ActivityDetails, ActivityFailure, ActivityFilters, ActivityResponse,
        CachedActivity, LoadProgress, PersonalApiKey,
    },
    identity::SourceId,
};
use garmin_service_api::ConnectActivityResult;
use garmin_storage::CachedAccount;

use super::{connect_source_id, original_fit, search};
use crate::{Application, Error, UserContext};

#[cfg(test)]
mod tests;

#[async_trait::async_trait]
trait Remote: Sync {
    fn account(&self) -> &AccountFacts;
    async fn activities(&self, start: u32) -> Result<Vec<CachedActivity>, garmin_connect::Error>;
    async fn download(&self, id: i64) -> Result<garmin_connect::Download, garmin_connect::Error>;
}

#[async_trait::async_trait]
impl Remote for garmin_connect::Session {
    fn account(&self) -> &AccountFacts {
        self.account()
    }
    async fn activities(&self, start: u32) -> Result<Vec<CachedActivity>, garmin_connect::Error> {
        self.activities(start).await
    }
    async fn download(&self, id: i64) -> Result<garmin_connect::Download, garmin_connect::Error> {
        self.download(id).await
    }
}

fn credential_hash(key: &PersonalApiKey) -> String {
    blake3::hash(key.expose().as_bytes()).to_hex().to_string()
}

impl Application {
    pub(super) async fn cached_connect_page(
        &self,
        user: UserContext,
        key: &PersonalApiKey,
        start: u32,
        filters: ActivityFilters,
        refresh: bool,
        progress: &(impl Fn(LoadProgress) + Send + Sync),
    ) -> Result<ConnectActivityResult, Error> {
        if !filters.valid() {
            return Err(garmin_connect::Error::InvalidSelection.into());
        }
        let fingerprint = credential_hash(key);
        let existing = self
            .storage
            .connect_cache_account(user.user_id(), &fingerprint)
            .await?;
        let mut warning = None;
        if refresh || existing.is_none() {
            let result = async {
                progress(LoadProgress::Connecting);
                let session = garmin_connect::Client::default().session(key).await?;
                self.load_connect_cache(user, &fingerprint, &session, progress)
                    .await
            }
            .await;
            if let Err(error) = result {
                if self
                    .storage
                    .connect_cache_account(user.user_id(), &fingerprint)
                    .await?
                    .is_none()
                {
                    return Err(error);
                }
                warning = Some(error.to_string());
            }
        }
        let account = self
            .storage
            .connect_cache_account(user.user_id(), &fingerprint)
            .await?
            .ok_or(garmin_connect::Error::MissingAccountIdentity)?;
        self.query_connect_cache(user, &account, start, filters, warning)
            .await
    }

    async fn query_connect_cache(
        &self,
        user: UserContext,
        account: &CachedAccount,
        start: u32,
        filters: ActivityFilters,
        warning: Option<String>,
    ) -> Result<ConnectActivityResult, Error> {
        let imported = self
            .storage
            .imported_source_identities(user.user_id(), account.source)
            .await?
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        let mut summaries = self
            .storage
            .connect_cache_summaries(user.user_id(), account)
            .await?;
        for summary in &mut summaries {
            summary.activity.imported = imported.contains(&summary.activity.id.to_string());
        }
        summaries.retain(|summary| search::matches(&filters, summary));
        summaries.sort_by(|left, right| {
            right
                .started_local
                .as_ref()
                .or(right.activity.started_at.as_ref())
                .cmp(
                    &left
                        .started_local
                        .as_ref()
                        .or(left.activity.started_at.as_ref()),
                )
                .then_with(|| right.activity.id.cmp(&left.activity.id))
        });
        let total = summaries.len();
        let start = usize::try_from(start)
            .map_err(|_| garmin_connect::Error::InvalidSelection)?
            .min(total.saturating_sub(1) / 20 * 20);
        let activities = summaries
            .into_iter()
            .skip(start)
            .take(20)
            .map(|summary| summary.activity)
            .collect::<Vec<_>>();
        let cache = self
            .storage
            .connect_cache_status(user.user_id(), account)
            .await?;
        Ok(ConnectActivityResult::State(ActivityResponse::Page {
            start: u32::try_from(start).map_err(|_| garmin_connect::Error::InvalidSelection)?,
            filters,
            has_more: start + activities.len() < total,
            total,
            cache,
            warning,
            activities,
        }))
    }

    async fn load_connect_cache(
        &self,
        user: UserContext,
        fingerprint: &str,
        remote: &impl Remote,
        progress: &(impl Fn(LoadProgress) + Send + Sync),
    ) -> Result<(), Error> {
        let account = CachedAccount {
            source: SourceId::from_u128(connect_source_id(user, remote.account())?.as_u128()),
            generation: uuid::Uuid::new_v4().to_string(),
            facts: remote.account().clone(),
        };
        let mut start = 0_u32;
        let mut seen = std::collections::BTreeMap::new();
        let mut pages = 0;
        progress(LoadProgress::Indexing {
            discovered: 0,
            pages,
        });
        loop {
            let summaries = remote.activities(start).await?;
            if summaries.len() > 20
                || summaries.iter().any(|summary| {
                    seen.insert(summary.activity.id, summary.activity.name.clone())
                        .is_some()
                })
            {
                return Err(garmin_connect::Error::InvalidActivities.into());
            }
            self.storage
                .stage_connect_summaries(
                    user.user_id(),
                    account.source,
                    &account.generation,
                    &summaries,
                )
                .await?;
            pages += 1;
            progress(LoadProgress::Indexing {
                discovered: seen.len(),
                pages,
            });
            if summaries.len() < 20 {
                break;
            }
            start = start
                .checked_add(20)
                .ok_or(garmin_connect::Error::InvalidSelection)?;
        }
        self.storage
            .publish_connect_cache(user.user_id(), fingerprint, &account)
            .await?;
        self.download_connect_originals(user, &account, seen, remote, progress)
            .await
    }

    async fn download_connect_originals(
        &self,
        user: UserContext,
        account: &CachedAccount,
        seen: std::collections::BTreeMap<i64, String>,
        remote: &impl Remote,
        progress: &(impl Fn(LoadProgress) + Send + Sync),
    ) -> Result<(), Error> {
        let status = self
            .storage
            .connect_cache_status(user.user_id(), account)
            .await?;
        let mut state = LoadProgress::Downloading {
            total: seen.len(),
            downloaded: 0,
            reused: status.downloaded,
            unavailable: 0,
            current: None,
        };
        progress(state.clone());
        if status.downloaded == seen.len() {
            return Ok(());
        }
        for (id, name) in seen.into_iter().rev() {
            if self
                .storage
                .connect_has_original(user.user_id(), account.source, id)
                .await?
            {
                continue;
            }
            if let LoadProgress::Downloading { current, .. } = &mut state {
                *current = Some(name);
            }
            progress(state.clone());
            let result = async {
                let download = remote.download(id).await?;
                if download.account.display_name != account.facts.display_name {
                    return Err(garmin_connect::Error::MissingAccountIdentity.into());
                }
                original_fit(&download.archive)?;
                Ok::<_, Error>(download.archive)
            }
            .await;
            let result = match result {
                Ok(bytes) => Ok(bytes),
                Err(error) => {
                    let failure = ActivityFailure::from(error);
                    if failure.stops_batch() {
                        return Err(Error::GarminConnectCache(failure.message));
                    }
                    Err(failure.message)
                }
            };
            self.storage
                .cache_connect_original(user.user_id(), account.source, id, &result)
                .await?;
            let finished = if let LoadProgress::Downloading {
                downloaded,
                unavailable,
                reused,
                total,
                current,
            } = &mut state
            {
                if result.is_ok() {
                    *downloaded += 1;
                } else {
                    *unavailable += 1;
                }
                *current = None;
                *downloaded + *unavailable + *reused == *total
            } else {
                false
            };
            progress(state.clone());
            if finished {
                break;
            }
        }
        Ok(())
    }

    pub(super) async fn cached_connect_download(
        &self,
        user: UserContext,
        key: &PersonalApiKey,
        id: i64,
    ) -> Result<(garmin_connect::Download, ActivityDetails), Error> {
        let account = self
            .storage
            .connect_cache_account(user.user_id(), &credential_hash(key))
            .await?
            .ok_or(garmin_connect::Error::InvalidSelection)?;
        let summary = self
            .storage
            .connect_cache_summaries(user.user_id(), &account)
            .await?
            .into_iter()
            .find(|summary| summary.activity.id == id)
            .ok_or(garmin_connect::Error::InvalidSelection)?;
        let archive = match self
            .storage
            .connect_cached_original(user.user_id(), account.source, id)
            .await?
        {
            Some(Ok(archive)) => archive,
            Some(Err(reason)) => return Err(Error::GarminConnectOriginalUnavailable(reason)),
            None => {
                let download = garmin_connect::Client::default().download(key, id).await?;
                if download.account.display_name != account.facts.display_name {
                    return Err(garmin_connect::Error::MissingAccountIdentity.into());
                }
                original_fit(&download.archive)?;
                self.storage
                    .cache_connect_original(
                        user.user_id(),
                        account.source,
                        id,
                        &Ok(download.archive.clone()),
                    )
                    .await?;
                download.archive
            }
        };
        Ok((
            garmin_connect::Download {
                account: account.facts,
                archive,
            },
            ActivityDetails {
                name: Some(summary.activity.name),
                description: summary.description,
            },
        ))
    }
}
