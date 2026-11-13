//! Cloud completions pass through the desktop controller, rather than calling Batch directly.
use super::*;
use garmin_model::garmin_connect::{ActivityRequest, ActivityResponse, RemoteActivity};
use garmin_service_api::ConnectActivityResult;

#[test]
fn page_import_updates_the_cached_page_and_list_requests_clear_its_summary()
-> Result<(), Box<dyn std::error::Error>> {
    let (_directory, mut desktop) = super::file_window_tests::desktop()?;
    let user = desktop.profiles[0].user.id();
    desktop.connect_activities.insert(
        user,
        ActivityResponse::Page {
            start: 0,
            has_more: true,
            activities: (1..=2)
                .map(|id| RemoteActivity {
                    id,
                    imported: false,
                    name: format!("Ride {id}"),
                    sport: None,
                    started_at: None,
                    distance_metres: None,
                    duration_seconds: None,
                })
                .collect(),
        },
    );
    desktop.start_page_import(0);
    assert!(desktop.preferences_saving);
    assert_eq!(desktop.connect_activity_loading, Some(user));
    assert!(desktop.connect_preview.is_none());
    desktop.connect_activity_checked(
        user,
        Ok(ConnectActivityResult::Prepared {
            id: 1,
            digest: "digest".into(),
            imported: false,
        }),
    );
    assert!(
        desktop.preferences_saving,
        "the Import request remains in flight"
    );
    // Switching away must not retarget the completed file or the next request.
    desktop.selected_profile = None;
    desktop.connect_activity_checked(
        user,
        Ok(ConnectActivityResult::State(ActivityResponse::Imported {
            id: 1,
            activities: 1,
            duplicate: false,
        })),
    );
    let batch = &desktop.page_import.as_ref().expect("batch").1;
    assert_eq!(batch.counts.imported, 1);
    assert_eq!(batch.current_name(), Some("Ride 2"));
    assert_eq!(desktop.connect_activity_loading, Some(user));
    let ActivityResponse::Page { activities, .. } = &desktop.connect_activities[&user] else {
        panic!("cached page");
    };
    assert!(activities[0].imported);
    assert!(!activities[1].imported);
    desktop.selected_profile = Some(0);
    desktop
        .handle_profile_settings_action(0, profile_settings::Action::CancelGarminConnectPageImport);
    desktop.connect_activity_checked(
        user,
        Ok(ConnectActivityResult::Prepared {
            id: 2,
            digest: "digest".into(),
            imported: false,
        }),
    );
    assert!(!desktop.preferences_saving);
    assert!(desktop.connect_activity_loading.is_none());
    assert!(desktop.page_import.as_ref().expect("summary").1.cancelled());
    desktop.handle_profile_settings_action(
        0,
        profile_settings::Action::GarminConnectActivity(ActivityRequest::List { start: 20 }),
    );
    assert!(
        desktop.page_import.is_none(),
        "a different page cannot inherit the old summary"
    );
    assert!(desktop.preferences_saving);
    Ok(())
}
