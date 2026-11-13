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
