//! Browser cloud-import transitions kept independent of the WebAssembly runtime.

use garmin_model::{
    garmin_connect::{ActivityFailure, ActivityRequest, ActivityResponse},
    identity::UserId,
};
use garmin_service_api::ConnectActivityResult;
use garmin_ui::profile_settings::connect_activities::Batch;
use std::collections::HashMap;
use std::rc::Rc;

pub enum Completion {
    Batch(Option<ActivityRequest>),
    Single(Result<ConnectActivityResult, ActivityFailure>),
}

/// Background reads are valid only until the next foreground mutation of that profile.
#[derive(Default)]
pub struct CacheReads {
    epochs: HashMap<UserId, u64>,
    applied: HashMap<UserId, garmin_model::garmin_connect::sync::Revision>,
}

pub struct CacheRead {
    user: UserId,
    epoch: u64,
    pub start: u32,
    pub filters: garmin_model::garmin_connect::ActivityFilters,
}

#[derive(Clone, Copy)]
pub struct ProfileEpoch {
    user: UserId,
    value: u64,
}

impl CacheReads {
    pub fn epoch(&self, user: UserId) -> ProfileEpoch {
        ProfileEpoch {
            user,
            value: self.epochs.get(&user).copied().unwrap_or_default(),
        }
    }

    pub fn is_current(&self, epoch: ProfileEpoch) -> bool {
        self.epochs.get(&epoch.user).copied().unwrap_or_default() == epoch.value
    }

    pub fn invalidate(&mut self, user: UserId) {
        let epoch = self.epochs.entry(user).or_default();
        *epoch = epoch.wrapping_add(1);
        self.applied.remove(&user);
    }

    pub fn changed(
        &self,
        user: UserId,
        revision: &garmin_model::garmin_connect::sync::Revision,
    ) -> bool {
        self.applied.get(&user) != Some(revision)
    }

    pub fn capture(&self, user: UserId, page: &ActivityResponse) -> Option<CacheRead> {
        let ActivityResponse::Page { start, filters, .. } = page else {
            return None;
        };
        Some(CacheRead {
            user,
            epoch: self.epochs.get(&user).copied().unwrap_or_default(),
            start: *start,
            filters: filters.clone(),
        })
    }

    pub fn apply(
        &mut self,
        request: CacheRead,
        revision: garmin_model::garmin_connect::sync::Revision,
        pages: &mut HashMap<UserId, ActivityResponse>,
        page: ActivityResponse,
    ) -> bool {
        let CacheRead {
            user,
            epoch,
            start,
            filters,
        } = request;
        if self.epochs.get(&user).copied().unwrap_or_default() != epoch
            || !matches!(pages.get(&user), Some(ActivityResponse::Page { start: current, filters: current_filters, .. }) if *current == start && *current_filters == filters)
        {
            return false;
        }
        pages.insert(user, page);
        self.applied.insert(user, revision);
        true
    }
}

/// A refresh warning must survive consuming the durable import's success receipt.
pub fn take_import_receipt(
    receipt: &mut Option<(UserId, ActivityResponse)>,
    stale: bool,
) -> Option<(UserId, ActivityResponse)> {
    receipt.take().filter(|_| !stale)
}

pub fn begin_request(
    batch: &mut Option<(UserId, Batch)>,
    request: &ActivityRequest,
    page: Option<&ActivityResponse>,
) {
    if garmin_ui::profile_settings::connect_activities::clears_batch(request, page) {
        *batch = None;
    }
}

pub fn complete(
    user: UserId,
    batch: &mut Option<(UserId, Batch)>,
    pages: &mut HashMap<UserId, ActivityResponse>,
    result: Result<ConnectActivityResult, ActivityFailure>,
) -> Completion {
    let Some((owner, batch)) = batch
        .as_mut()
        .filter(|(owner, batch)| *owner == user && batch.running())
    else {
        return Completion::Single(result);
    };
    let step = batch.complete(result);
    if let Some(receipt) = step.receipt
        && let Some(page) = pages.get_mut(owner)
    {
        page.merge(receipt);
    }
    Completion::Batch(step.request)
}

/// Preserve the previous local list on refresh failure and retain an explicit retry requirement.
pub fn finish_refresh<T, E>(
    profiles: &mut Rc<Vec<T>>,
    stale: &mut bool,
    refresh: Option<Result<Vec<T>, E>>,
) -> Option<E> {
    match refresh {
        Some(Ok(next)) => {
            *profiles = Rc::new(next);
            *stale = false;
            None
        }
        Some(Err(error)) => {
            *stale = true;
            Some(error)
        }
        None => None,
    }
}
