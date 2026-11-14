#[path = "../src/connect_state.rs"]
mod connect_state;

use garmin_model::{
    garmin_connect::{ActivityRequest, ActivityResponse, RemoteActivity},
    identity::UserId,
};
use garmin_service_api::ConnectActivityResult;
use garmin_ui::profile_settings::connect_activities::Batch;

fn row(id: i64) -> RemoteActivity {
    RemoteActivity {
        id,
        imported: false,
        name: format!("Ride {id}"),
        sport: None,
        started_at: None,
        distance_metres: None,
        duration_seconds: None,
    }
}

fn cached_pages(
    owner: UserId,
    other: UserId,
) -> std::collections::HashMap<UserId, ActivityResponse> {
    std::collections::HashMap::from([
        (
            owner,
            ActivityResponse::Page {
                start: 0,
                filters: garmin_model::garmin_connect::ActivityFilters::default(),
                total: 2,
                cache: garmin_model::garmin_connect::CacheStatus::default(),
                warning: None,
                has_more: false,
                activities: vec![row(1), row(2)],
            },
        ),
        (
            other,
            ActivityResponse::Page {
                start: 0,
                filters: garmin_model::garmin_connect::ActivityFilters::default(),
                total: 1,
                cache: garmin_model::garmin_connect::CacheStatus::default(),
                warning: None,
                has_more: false,
                activities: vec![row(1)],
            },
        ),
    ])
}

#[test]
fn browser_routes_import_receipts_and_next_requests_to_the_original_profile() {
    let owner = UserId::from_u128(1);
    let other = UserId::from_u128(2);
    let mut batch = Batch::new(&[row(1), row(2)]).unwrap();
    assert_eq!(
        batch.next_request(),
        Some(ActivityRequest::Prepare { id: 1 })
    );
    let mut batch = Some((owner, batch));
    let mut pages = cached_pages(owner, other);
    let result = connect_state::complete(
        owner,
        &mut batch,
        &mut pages,
        Ok(ConnectActivityResult::Prepared {
            id: 1,
            digest: "exact-fit".into(),
            imported: false,
        }),
    );
    assert!(
        matches!(result, connect_state::Completion::Batch(Some(ActivityRequest::Import { id: 1, digest })) if digest == "exact-fit")
    );
    let result = connect_state::complete(
        owner,
        &mut batch,
        &mut pages,
        Ok(ConnectActivityResult::State(ActivityResponse::Imported {
            id: 1,
            activities: 1,
            duplicate: false,
        })),
    );
    assert!(matches!(
        result,
        connect_state::Completion::Batch(Some(ActivityRequest::Prepare { id: 2 }))
    ));
    let ActivityResponse::Page { activities, .. } = &pages[&owner] else {
        panic!("owner page");
    };
    assert!(activities[0].imported);
    assert!(!activities[1].imported);
    let ActivityResponse::Page { activities, .. } = &pages[&other] else {
        panic!("other page");
    };
    assert!(!activities[0].imported);
    batch.as_mut().unwrap().1.cancel();
    let result = connect_state::complete(
        owner,
        &mut batch,
        &mut pages,
        Ok(ConnectActivityResult::Prepared {
            id: 2,
            digest: "cancelled-fit".into(),
            imported: false,
        }),
    );
    assert!(matches!(result, connect_state::Completion::Batch(None)));
    assert_eq!(batch.as_ref().unwrap().1.counts.imported, 1);
    connect_state::begin_request(
        &mut batch,
        &ActivityRequest::List {
            start: 0,
            filters: garmin_model::garmin_connect::ActivityFilters::default(),
        },
        pages.get(&owner),
    );
    assert!(
        batch.is_some(),
        "refilling the same cached query retains its import summary"
    );
    connect_state::begin_request(
        &mut batch,
        &ActivityRequest::List {
            start: 20,
            filters: garmin_model::garmin_connect::ActivityFilters::default(),
        },
        None,
    );
    assert!(batch.is_none());
}

#[test]
fn another_profiles_response_does_not_advance_the_batch() {
    let owner = UserId::from_u128(1);
    let mut batch = Batch::new(&[row(1)]).unwrap();
    assert!(batch.next_request().is_some());
    let mut batch = Some((owner, batch));
    let failure = garmin_model::garmin_connect::ActivityFailure {
        message: "other profile failure".into(),
        kind: garmin_model::garmin_connect::ActivityFailureKind::Session,
    };
    let result = connect_state::complete(
        UserId::from_u128(2),
        &mut batch,
        &mut std::collections::HashMap::new(),
        Err(failure),
    );
    assert!(matches!(result, connect_state::Completion::Single(Err(_))));
    assert!(batch.as_ref().unwrap().1.running());
    assert_eq!(batch.as_ref().unwrap().1.counts.processed(), 0);
}

#[test]
fn refresh_failure_preserves_the_local_list_and_remains_retryable() {
    let mut profiles = std::rc::Rc::new(vec!["previous activity"]);
    let mut stale = false;
    assert_eq!(
        connect_state::finish_refresh(
            &mut profiles,
            &mut stale,
            Some(Err("connection interrupted"))
        ),
        Some("connection interrupted")
    );
    assert!(stale);
    assert_eq!(&*profiles, &["previous activity"]);
    let mut receipt = Some((
        UserId::from_u128(1),
        ActivityResponse::Imported {
            id: 42,
            activities: 1,
            duplicate: false,
        },
    ));
    assert!(
        connect_state::take_import_receipt(&mut receipt, stale).is_none(),
        "import success must not replace the refresh warning"
    );
    assert!(
        receipt.is_none(),
        "a consumed receipt cannot later hide the warning"
    );
    // An unrelated response must not discard the retry requirement.
    assert_eq!(
        connect_state::finish_refresh::<_, &str>(&mut profiles, &mut stale, None),
        None
    );
    assert!(stale);
    assert_eq!(
        connect_state::finish_refresh::<_, &str>(
            &mut profiles,
            &mut stale,
            Some(Ok(vec!["previous activity", "imported activity"]))
        ),
        None
    );
    assert!(!stale);
    assert_eq!(&*profiles, &["previous activity", "imported activity"]);
    let mut receipt = Some((
        UserId::from_u128(1),
        ActivityResponse::Imported {
            id: 43,
            activities: 1,
            duplicate: false,
        },
    ));
    assert!(
        connect_state::take_import_receipt(&mut receipt, stale).is_some(),
        "a later successful import can announce success after the local list recovers"
    );
}
