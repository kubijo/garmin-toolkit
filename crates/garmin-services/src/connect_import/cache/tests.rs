use std::sync::Mutex;

use super::*;
use garmin_model::{
    activity::{ActivityDuration, ActivitySport, Distance},
    garmin_connect::{ActivityRequest, RemoteActivity},
    identity::{MonthlyDistanceTarget, Profile, Role, User, UserId},
};

struct MockRemote {
    facts: AccountFacts,
    summaries: Vec<CachedActivity>,
    archive: Vec<u8>,
    fail: Option<(i64, u16)>,
    pages: Mutex<Vec<u32>>,
    downloads: Mutex<Vec<i64>>,
    gate: Option<std::sync::Arc<DownloadGate>>,
}

#[derive(Default)]
struct DownloadGate {
    started: tokio::sync::Notify,
    finish: tokio::sync::Notify,
}

fn uninterrupted() -> Control<'static> {
    Control::new(&|_| {}, &|| false)
}

impl MockRemote {
    fn new() -> Self {
        use std::io::Write;
        let bytes = garmin_fit::fixture::activity(garmin_fit::fixture::Sport::Cycling).unwrap();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file("activity.fit", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&bytes).unwrap();
        Self {
            facts: AccountFacts {
                display_name: Some("account".into()),
                locale: None,
                measurement_system: None,
                time_zone: None,
            },
            summaries: (1..=23)
                .map(|id| CachedActivity {
                    activity: RemoteActivity {
                        id,
                        imported: false,
                        sport: Some(ActivitySport::Cycling),
                        name: if id == 23 {
                            "Hamina Cycling".into()
                        } else {
                            format!("Ride {id}")
                        },
                        started_at: Some("2026-11-10 07:12:38".into()),
                        distance: Distance::try_from_metres(1000.0),
                        duration: ActivityDuration::try_from_seconds(60.0),
                    },
                    description: Some(if id == 23 {
                        "Coastal recovery ride".into()
                    } else {
                        "Forest loop".into()
                    }),
                    activity_type: Some("road_cycling".into()),
                    started_local: Some(if id == 23 {
                        "2026-11-11 00:30:00".into()
                    } else {
                        "2026-11-10 09:12:38".into()
                    }),
                })
                .collect(),
            archive: writer.finish().unwrap().into_inner(),
            fail: Some((1, 404)),
            pages: Mutex::new(Vec::new()),
            downloads: Mutex::new(Vec::new()),
            gate: None,
        }
    }
}

#[async_trait::async_trait]
impl Remote for MockRemote {
    fn account(&self) -> &AccountFacts {
        &self.facts
    }
    async fn activities(&self, start: u32) -> Result<Vec<CachedActivity>, garmin_connect::Error> {
        self.pages.lock().unwrap().push(start);
        Ok(self
            .summaries
            .iter()
            .skip(start as usize)
            .take(20)
            .cloned()
            .collect())
    }
    async fn download(&self, id: i64) -> Result<garmin_connect::Download, garmin_connect::Error> {
        self.downloads.lock().unwrap().push(id);
        if let Some(gate) = &self.gate {
            gate.started.notify_one();
            gate.finish.notified().await;
        }
        if let Some((failed, status)) = self.fail
            && failed == id
        {
            return Err(garmin_connect::Error::Rejected {
                operation: "activity download",
                status,
            });
        }
        Ok(garmin_connect::Download {
            account: self.facts.clone(),
            archive: self.archive.clone(),
        })
    }
}

async fn application(path: &std::path::Path) -> (Application, UserContext, PersonalApiKey) {
    let storage = garmin_storage::Storage::open(path).await.unwrap();
    let user = User::from_parts(
        UserId::from_u128(1),
        Role::Owner,
        Profile::from_display_name("Rider".parse().unwrap()),
    );
    storage.save_user(&user).await.unwrap();
    let key = PersonalApiKey::from_string("test-account-key".into()).unwrap();
    storage
        .set_garmin_connect_key(user.id(), Some(&key))
        .await
        .unwrap();
    (Application::new(storage), UserContext::new(user.id()), key)
}

#[tokio::test]
async fn trends_targets_survive_preferences_persistence_and_restart() {
    let root = tempfile::tempdir().expect("directory");
    let path = root.path().join("targets.sqlite3");
    let (app, user, _) = application(&path).await;
    let targets = [160, 500, 10].map(MonthlyDistanceTarget::from_kilometres);
    app.update_profile_preferences(
        user,
        garmin_model::identity::ProfilePreferences::default()
            .with_monthly_distance_targets(targets),
    )
    .await
    .expect("save preferences");
    drop(app);
    let storage = garmin_storage::Storage::open(&path).await.expect("reopen");
    assert_eq!(
        storage
            .user(user.user_id())
            .await
            .expect("stored user")
            .expect("user exists")
            .profile()
            .preferences()
            .monthly_distance_targets(),
        targets
    );
}

#[tokio::test]
async fn trends_read_all_published_summaries_without_imports_or_refreshes() {
    let root = tempfile::tempdir().expect("directory");
    let (app, user, key) = application(&root.path().join("trends.sqlite3")).await;
    assert!(
        app.connect_trends(user)
            .await
            .expect("empty cache")
            .is_none()
    );
    let remote = MockRemote::new();
    app.load_connect_cache(
        user,
        &credential_hash(&key),
        &remote,
        Refresh::Full,
        &uninterrupted(),
    )
    .await
    .expect("cache");
    let report = app
        .connect_trends(user)
        .await
        .expect("read trends")
        .expect("published history");
    assert_eq!(
        report.summaries, 23,
        "includes summaries beyond the first page"
    );
    let range = garmin_model::garmin_connect::trends::Range {
        from: "2026-11-01".parse().expect("date"),
        through: "2026-11-30".parse().expect("date"),
    };
    assert_eq!(report.totals(range, None).activities, 23);
    assert_eq!(report.totals(range, None).elevation_activities, 0);
    assert_eq!(report.totals(range, None).heart_rate.activities(), 0);
    assert_eq!(app.activities(user).await.expect("imports").len(), 0);
    let sync_revision = app.connect_sync_status(user).await.unwrap().revision();
    let import_revision = app.connect_trends_import_revision(user).await.unwrap();
    import_cached_preview(&app, user).await;
    assert_eq!(
        app.connect_sync_status(user).await.unwrap().revision(),
        sync_revision
    );
    assert_ne!(
        app.connect_trends_import_revision(user).await.unwrap(),
        import_revision
    );
    let report = app
        .connect_trends(user)
        .await
        .expect("read trends")
        .expect("published history");
    let totals = report.totals(range, None);
    assert_eq!(totals.activities, 23);
    assert_eq!(totals.elevation_activities, 1);
    let imported = app.activities(user).await.expect("FIT projections");
    assert_eq!(totals.heart_rate.activities(), 1);
    assert_eq!(
        totals.heart_rate.value(),
        imported[0].summary().metrics().average_heart_rate()
    );
    assert_eq!(
        Some(totals.elevation_gain),
        imported[0].summary().totals().ascent()
    );
    assert_eq!(*remote.pages.lock().expect("pages"), [0, 20]);
    assert!(
        app.connect_trends(UserContext::new(UserId::new_v4()))
            .await
            .is_err()
    );
    let replacement = PersonalApiKey::from_string("different-key".into()).expect("key");
    app.storage
        .set_garmin_connect_key(user.user_id(), Some(&replacement))
        .await
        .expect("replace");
    assert!(
        app.connect_trends(user)
            .await
            .expect("scoped cache")
            .is_none()
    );
}

#[tokio::test]
async fn cancelled_initial_load_returns_a_normal_outcome_without_contacting_garmin() {
    let root = tempfile::tempdir().unwrap();
    let (app, user, _) = application(&root.path().join("cache.sqlite3")).await;
    let result = app
        .garmin_connect_activity_cancellable(
            user,
            ActivityRequest::Load {
                start: 0,
                filters: ActivityFilters::default(),
            },
            |_| {},
            || true,
        )
        .await
        .unwrap();
    assert!(matches!(
        result,
        ConnectActivityResult::RefreshCancelled(None)
    ));
    assert_eq!(app.activities(user).await.unwrap().len(), 0);
}

#[tokio::test]
async fn cancelling_an_index_keeps_the_previous_snapshot_even_on_the_final_page() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cache.sqlite3");
    let (app, user, key) = application(&path).await;
    let database = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .read_only(true),
    )
    .await
    .unwrap();
    let mut remote = MockRemote::new();
    let fingerprint = credential_hash(&key);
    app.load_connect_cache(user, &fingerprint, &remote, Refresh::Full, &uninterrupted())
        .await
        .unwrap();
    let previous = cached_account(&app, user, &fingerprint).await;
    let summaries = remote.summaries.clone();
    for count in [1, 23] {
        remote.summaries = summaries[..count].to_vec();
        remote.summaries[0].activity.name = "Unpublished edit".into();
        remote.pages.lock().unwrap().clear();
        remote.downloads.lock().unwrap().clear();
        let cancel = garmin_progress::CancellationToken::default();
        let report = |progress| {
            if matches!(progress, LoadProgress::Indexing { pages: 1, .. }) {
                cancel.cancel();
            }
        };
        let result = app
            .load_connect_cache(
                user,
                &fingerprint,
                &remote,
                Refresh::Full,
                &Control::new(&report, &|| cancel.is_cancelled()),
            )
            .await;
        assert!(matches!(result, Err(Error::GarminConnectRefreshCancelled)));
        assert_eq!(*remote.pages.lock().unwrap(), [0]);
        assert!(remote.downloads.lock().unwrap().is_empty());
        let retained = cached_account(&app, user, &fingerprint).await;
        assert_eq!(retained.generation, previous.generation);
        let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM connect_cache_summaries")
            .fetch_one(&database)
            .await
            .unwrap();
        assert_eq!(
            stored, 23,
            "cancelled generations must be physically removed"
        );
        assert_eq!(
            app.storage
                .connect_cache_status(user.user_id(), &retained)
                .await
                .unwrap()
                .total,
            23
        );
    }
}

#[tokio::test]
async fn cancellation_finishes_the_in_flight_file_and_resumes_without_redownloading_it() {
    let root = tempfile::tempdir().unwrap();
    let (app, user, key) = application(&root.path().join("cache.sqlite3")).await;
    let mut remote = MockRemote::new();
    remote.fail = None;
    let gate = std::sync::Arc::new(DownloadGate::default());
    remote.gate = Some(gate.clone());
    let cancel = garmin_progress::CancellationToken::default();
    let cancelled = || cancel.is_cancelled();
    let report = |_| {};
    let control = Control::new(&report, &cancelled);
    let fingerprint = credential_hash(&key);
    let load = app.load_connect_cache(user, &fingerprint, &remote, Refresh::Full, &control);
    let stop = async {
        gate.started.notified().await;
        cancel.cancel();
        gate.finish.notify_one();
    };
    let (result, ()) = tokio::join!(load, stop);
    assert!(matches!(result, Err(Error::GarminConnectRefreshCancelled)));
    assert_eq!(*remote.downloads.lock().unwrap(), [23]);
    let response = app
        .cancelled_connect_page(user, &key, 0, ActivityFilters::default())
        .await
        .unwrap();
    let ConnectActivityResult::RefreshCancelled(Some(ActivityResponse::Page { cache, .. })) =
        response
    else {
        panic!("cancelled refresh returns the published page")
    };
    assert_eq!((cache.total, cache.downloaded), (23, 1));
    remote.gate = None;
    app.load_connect_cache(
        user,
        &fingerprint,
        &remote,
        Refresh::Recent,
        &uninterrupted(),
    )
    .await
    .unwrap();
    assert_eq!(
        remote
            .downloads
            .lock()
            .unwrap()
            .iter()
            .filter(|id| **id == 23)
            .count(),
        1
    );
    let account = cached_account(&app, user, &fingerprint).await;
    assert_eq!(
        app.storage
            .connect_cache_status(user.user_id(), &account)
            .await
            .unwrap()
            .downloaded,
        23
    );
    assert_eq!(app.activities(user).await.unwrap().len(), 0);
}

#[tokio::test]
async fn cancelling_a_queued_request_preserves_the_index_without_waiting_for_the_lock() {
    let root = tempfile::tempdir().unwrap();
    let (app, user, key) = application(&root.path().join("cache.sqlite3")).await;
    let remote = MockRemote::new();
    let fingerprint = credential_hash(&key);
    app.load_connect_cache(user, &fingerprint, &remote, Refresh::Full, &uninterrupted())
        .await
        .unwrap();
    let lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    app.connect_cache_locks
        .lock()
        .unwrap()
        .insert(user.user_id(), lock.clone());
    let _guard = lock.lock().await;
    let cancel = garmin_progress::CancellationToken::default();
    let waiting = tokio::sync::Notify::new();
    let request = app.garmin_connect_activity_cancellable(
        user,
        ActivityRequest::Load {
            start: 0,
            filters: ActivityFilters::default(),
        },
        |progress| {
            if matches!(progress, LoadProgress::Waiting) {
                waiting.notify_one();
            }
        },
        || cancel.is_cancelled(),
    );
    let stop = async {
        waiting.notified().await;
        cancel.cancel();
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(request, stop)
    })
    .await
    .unwrap();
    assert!(matches!(
        result.unwrap(),
        ConnectActivityResult::RefreshCancelled(None)
    ));
    let account = cached_account(&app, user, &fingerprint).await;
    assert_eq!(
        app.storage
            .connect_cache_status(user.user_id(), &account)
            .await
            .unwrap()
            .total,
        23
    );
}

async fn cached_account(app: &Application, user: UserContext, fingerprint: &str) -> CachedAccount {
    app.storage
        .connect_cache_account(user.user_id(), fingerprint)
        .await
        .unwrap()
        .unwrap()
}

async fn import_cached_preview(app: &Application, user: UserContext) {
    let preview = app
        .garmin_connect_activity(user, ActivityRequest::Preview { id: 23 })
        .await
        .unwrap();
    let ConnectActivityResult::Preview(preview) = preview else {
        panic!("expected cached FIT preview")
    };
    assert_eq!(
        preview.details.description.as_deref(),
        Some("Coastal recovery ride")
    );
    assert_eq!(app.activities(user).await.unwrap().len(), 0);
    app.garmin_connect_activity(
        user,
        ActivityRequest::Import {
            id: 23,
            digest: preview.digest,
        },
    )
    .await
    .unwrap();
    assert_eq!(app.activities(user).await.unwrap().len(), 1);
}

#[tokio::test]
async fn full_account_cache_survives_restart_and_searches_beyond_page_one_without_importing() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cache.sqlite3");
    let (app, user, key) = application(&path).await;
    let remote = MockRemote::new();
    let fingerprint = credential_hash(&key);
    app.load_connect_cache(user, &fingerprint, &remote, Refresh::Full, &uninterrupted())
        .await
        .unwrap();
    assert_eq!(*remote.pages.lock().unwrap(), [0, 20]);
    assert_eq!(remote.downloads.lock().unwrap().len(), 23);
    assert_eq!(
        app.activities(user).await.unwrap().len(),
        0,
        "downloads are not imports"
    );
    drop(app);
    let app = Application::new(garmin_storage::Storage::open(&path).await.unwrap());
    let account = cached_account(&app, user, &fingerprint).await;
    assert!(
        app.storage
            .connect_cache_account(UserId::from_u128(2), &fingerprint)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        app.storage
            .connect_cache_account(user.user_id(), "another-key")
            .await
            .unwrap()
            .is_none()
    );
    let result = app
        .query_connect_cache(
            user,
            &account,
            0,
            ActivityFilters {
                search: "hamnia coastl".into(),
                from: Some("2026-11-11".parse().unwrap()),
                through: Some("2026-11-11".parse().unwrap()),
                ..ActivityFilters::default()
            },
            None,
        )
        .await
        .unwrap();
    let ConnectActivityResult::State(ActivityResponse::Page {
        activities,
        total,
        cache,
        has_more,
        ..
    }) = result
    else {
        panic!("expected cached page")
    };
    assert_eq!(total, 1);
    assert!(!has_more);
    assert_eq!(activities[0].id, 23);
    assert_eq!(
        (cache.total, cache.downloaded, cache.unavailable),
        (23, 22, 1)
    );
    import_cached_preview(&app, user).await;
    app.load_connect_cache(user, &fingerprint, &remote, Refresh::Full, &uninterrupted())
        .await
        .unwrap();
    assert_eq!(
        remote.downloads.lock().unwrap().len(),
        24,
        "refresh retries only the unavailable original"
    );
    let account = cached_account(&app, user, &fingerprint).await;
    let result = app
        .query_connect_cache(
            user,
            &account,
            0,
            ActivityFilters {
                search: "hamnia".into(),
                not_imported: true,
                ..ActivityFilters::default()
            },
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        result,
        ConnectActivityResult::State(ActivityResponse::Page { total: 0, .. })
    ));
}

#[tokio::test]
async fn progress_tracks_indexing_downloads_reuse_and_unavailable_originals() {
    let root = tempfile::tempdir().unwrap();
    let (app, user, key) = application(&root.path().join("cache.sqlite3")).await;
    let remote = MockRemote::new();
    let fingerprint = credential_hash(&key);
    let updates = Mutex::new(Vec::new());
    let report = |value| {
        if let LoadProgress::Downloading {
            downloaded,
            unavailable,
            current: Some(_),
            ..
        } = &value
        {
            assert_eq!(
                remote.downloads.lock().unwrap().len(),
                downloaded + unavailable,
                "the current file must be reported before its download starts"
            );
        }
        updates.lock().unwrap().push(value);
    };
    app.load_connect_cache(
        user,
        &fingerprint,
        &remote,
        Refresh::Full,
        &Control::new(&report, &|| false),
    )
    .await
    .unwrap();
    {
        let updates = updates.lock().unwrap();
        assert!(updates.contains(&LoadProgress::Indexing {
            discovered: 20,
            pages: 1
        }));
        assert!(updates.contains(&LoadProgress::Indexing {
            discovered: 23,
            pages: 2
        }));
        assert!(matches!(
            updates.last(),
            Some(LoadProgress::Downloading {
                total: 23,
                downloaded: 22,
                reused: 0,
                unavailable: 1,
                current: None,
            })
        ));
        let mut previous = 0;
        for update in updates.iter() {
            if let LoadProgress::Downloading {
                total,
                downloaded,
                reused,
                unavailable,
                ..
            } = update
            {
                let completed = downloaded + reused + unavailable;
                assert!(completed >= previous && completed <= *total);
                previous = completed;
            }
        }
    }
    updates.lock().unwrap().clear();
    remote.downloads.lock().unwrap().clear();
    app.load_connect_cache(
        user,
        &fingerprint,
        &remote,
        Refresh::Full,
        &Control::new(&report, &|| false),
    )
    .await
    .unwrap();
    assert!(
        updates
            .lock()
            .unwrap()
            .contains(&LoadProgress::Downloading {
                total: 23,
                downloaded: 0,
                reused: 22,
                unavailable: 0,
                current: None,
            })
    );
    assert!(matches!(
        updates.lock().unwrap().last(),
        Some(LoadProgress::Downloading {
            total: 23,
            downloaded: 0,
            reused: 22,
            unavailable: 1,
            current: None,
        })
    ));
}

#[tokio::test]
async fn rate_limits_leave_searchable_metadata_and_resume_without_redownloading_completed_files() {
    let root = tempfile::tempdir().unwrap();
    let (app, user, key) = application(&root.path().join("cache.sqlite3")).await;
    let mut remote = MockRemote::new();
    remote.fail = Some((22, 429));
    let fingerprint = credential_hash(&key);
    assert!(
        app.load_connect_cache(user, &fingerprint, &remote, Refresh::Full, &uninterrupted())
            .await
            .is_err()
    );
    let account = cached_account(&app, user, &fingerprint).await;
    assert_eq!(
        app.storage
            .connect_cache_summaries(user.user_id(), &account)
            .await
            .unwrap()
            .len(),
        23
    );
    assert_eq!(*remote.downloads.lock().unwrap(), [23, 22]);
    remote.fail = None;
    remote.pages.lock().unwrap().clear();
    app.load_connect_cache(
        user,
        &fingerprint,
        &remote,
        Refresh::Recent,
        &uninterrupted(),
    )
    .await
    .unwrap();
    assert_eq!(*remote.pages.lock().unwrap(), [0]);
    assert_eq!(
        remote
            .downloads
            .lock()
            .unwrap()
            .iter()
            .filter(|id| **id == 23)
            .count(),
        1
    );
    let account = cached_account(&app, user, &fingerprint).await;
    assert_eq!(
        app.storage
            .connect_cache_status(user.user_id(), &account)
            .await
            .unwrap()
            .downloaded,
        23
    );
    assert_eq!(app.activities(user).await.unwrap().len(), 0);
}

#[tokio::test]
async fn incremental_refresh_preserves_history_retries_missing_files_and_full_refresh_reconciles_it()
 {
    let root = tempfile::tempdir().unwrap();
    let (app, user, key) = application(&root.path().join("cache.sqlite3")).await;
    let mut remote = MockRemote::new();
    let fingerprint = credential_hash(&key);
    app.load_connect_cache(user, &fingerprint, &remote, Refresh::Full, &uninterrupted())
        .await
        .unwrap();
    let mut newest = remote.summaries[0].clone();
    newest.activity.id = 24;
    newest.activity.name = "New ride".into();
    remote.summaries.insert(0, newest);
    remote.summaries.retain(|summary| summary.activity.id != 23);
    remote
        .summaries
        .iter_mut()
        .find(|summary| summary.activity.id == 5)
        .unwrap()
        .activity
        .name = "Recent edit".into();
    remote
        .summaries
        .iter_mut()
        .find(|summary| summary.activity.id == 22)
        .unwrap()
        .activity
        .name = "Older edit".into();
    remote.fail = None;
    remote.pages.lock().unwrap().clear();
    remote.downloads.lock().unwrap().clear();
    app.load_connect_cache(
        user,
        &fingerprint,
        &remote,
        Refresh::Recent,
        &uninterrupted(),
    )
    .await
    .unwrap();
    assert_eq!(*remote.pages.lock().unwrap(), [0]);
    assert_eq!(*remote.downloads.lock().unwrap(), [24, 1]);
    let account = cached_account(&app, user, &fingerprint).await;
    let summaries = app
        .storage
        .connect_cache_summaries(user.user_id(), &account)
        .await
        .unwrap();
    assert_eq!(summaries.len(), 24);
    assert_eq!(
        summaries
            .iter()
            .find(|summary| summary.activity.id == 5)
            .unwrap()
            .activity
            .name,
        "Recent edit"
    );
    assert_eq!(
        summaries
            .iter()
            .find(|summary| summary.activity.id == 22)
            .unwrap()
            .activity
            .name,
        "Ride 22"
    );
    remote.pages.lock().unwrap().clear();
    remote.downloads.lock().unwrap().clear();
    app.load_connect_cache(user, &fingerprint, &remote, Refresh::Full, &uninterrupted())
        .await
        .unwrap();
    assert_eq!(*remote.pages.lock().unwrap(), [0, 20]);
    assert!(remote.downloads.lock().unwrap().is_empty());
    let account = cached_account(&app, user, &fingerprint).await;
    let summaries = app
        .storage
        .connect_cache_summaries(user.user_id(), &account)
        .await
        .unwrap();
    assert_eq!(summaries.len(), 23);
    assert_eq!(
        summaries
            .iter()
            .find(|summary| summary.activity.id == 22)
            .unwrap()
            .activity
            .name,
        "Older edit"
    );
    assert!(
        !app.storage
            .connect_has_original(user.user_id(), account.source, 23)
            .await
            .unwrap()
    );
    assert_eq!(app.activities(user).await.unwrap().len(), 0);
}

#[tokio::test]
async fn incremental_refresh_reads_all_new_pages_before_the_first_overlap() {
    let root = tempfile::tempdir().unwrap();
    let (app, user, key) = application(&root.path().join("cache.sqlite3")).await;
    let mut remote = MockRemote::new();
    remote.fail = None;
    let fingerprint = credential_hash(&key);
    app.load_connect_cache(user, &fingerprint, &remote, Refresh::Full, &uninterrupted())
        .await
        .unwrap();
    let template = remote.summaries[0].clone();
    let mut new = (24..=48)
        .map(|id| {
            let mut summary = template.clone();
            summary.activity.id = id;
            summary
        })
        .collect::<Vec<_>>();
    new.append(&mut remote.summaries);
    remote.summaries = new;
    remote.pages.lock().unwrap().clear();
    remote.downloads.lock().unwrap().clear();
    app.load_connect_cache(
        user,
        &fingerprint,
        &remote,
        Refresh::Recent,
        &uninterrupted(),
    )
    .await
    .unwrap();
    assert_eq!(*remote.pages.lock().unwrap(), [0, 20]);
    assert_eq!(remote.downloads.lock().unwrap().len(), 25);
    let account = cached_account(&app, user, &fingerprint).await;
    assert_eq!(
        app.storage
            .connect_cache_status(user.user_id(), &account)
            .await
            .unwrap()
            .total,
        48
    );
}

#[tokio::test]
async fn failed_metadata_refresh_preserves_the_previous_snapshot_and_account_switch_cannot_reuse_its_files()
 {
    let root = tempfile::tempdir().unwrap();
    let (app, user, key) = application(&root.path().join("cache.sqlite3")).await;
    let mut remote = MockRemote::new();
    let fingerprint = credential_hash(&key);
    app.load_connect_cache(user, &fingerprint, &remote, Refresh::Full, &uninterrupted())
        .await
        .unwrap();
    let previous = cached_account(&app, user, &fingerprint).await;
    remote.summaries[20] = remote.summaries[0].clone();
    assert!(
        app.load_connect_cache(user, &fingerprint, &remote, Refresh::Full, &uninterrupted())
            .await
            .is_err()
    );
    let retained = cached_account(&app, user, &fingerprint).await;
    assert_eq!(retained.generation, previous.generation);
    assert_eq!(
        app.storage
            .connect_cache_summaries(user.user_id(), &retained)
            .await
            .unwrap()
            .len(),
        23
    );
    remote.summaries.truncate(1);
    remote.fail = None;
    remote.facts.display_name = Some("different-account".into());
    let count = remote.downloads.lock().unwrap().len();
    app.load_connect_cache(
        user,
        "replacement-key",
        &remote,
        Refresh::Full,
        &uninterrupted(),
    )
    .await
    .unwrap();
    let switched = app
        .storage
        .connect_cache_account(user.user_id(), "replacement-key")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(switched.source, retained.source);
    assert_eq!(remote.downloads.lock().unwrap().len(), count + 1);
    assert!(
        !app.storage
            .connect_has_original(user.user_id(), retained.source, 23)
            .await
            .unwrap()
    );
    assert!(
        app.storage
            .connect_cache_account(user.user_id(), &fingerprint)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn interrupted_incremental_indexing_keeps_the_previous_snapshot_and_downloads_nothing() {
    let root = tempfile::tempdir().unwrap();
    let (app, user, key) = application(&root.path().join("cache.sqlite3")).await;
    let mut remote = MockRemote::new();
    let fingerprint = credential_hash(&key);
    app.load_connect_cache(user, &fingerprint, &remote, Refresh::Full, &uninterrupted())
        .await
        .unwrap();
    let previous = cached_account(&app, user, &fingerprint).await;
    let template = remote.summaries[0].clone();
    remote.summaries = (24..=48)
        .map(|id| {
            let mut summary = template.clone();
            summary.activity.id = id;
            summary
        })
        .collect();
    remote.summaries[20] = remote.summaries[0].clone();
    remote.downloads.lock().unwrap().clear();
    assert!(
        app.load_connect_cache(
            user,
            &fingerprint,
            &remote,
            Refresh::Recent,
            &uninterrupted()
        )
        .await
        .is_err()
    );
    let retained = cached_account(&app, user, &fingerprint).await;
    assert_eq!(retained.generation, previous.generation);
    assert_eq!(
        app.storage
            .connect_cache_status(user.user_id(), &retained)
            .await
            .unwrap()
            .total,
        23
    );
    assert!(remote.downloads.lock().unwrap().is_empty());
}

#[tokio::test]
async fn incremental_request_for_a_different_account_falls_back_to_full_enumeration() {
    let root = tempfile::tempdir().unwrap();
    let (app, user, key) = application(&root.path().join("cache.sqlite3")).await;
    let mut remote = MockRemote::new();
    remote.fail = None;
    let fingerprint = credential_hash(&key);
    app.load_connect_cache(
        user,
        &fingerprint,
        &remote,
        Refresh::Recent,
        &uninterrupted(),
    )
    .await
    .unwrap();
    let original = cached_account(&app, user, &fingerprint).await;
    remote.facts.display_name = Some("different-account".into());
    remote.pages.lock().unwrap().clear();
    remote.downloads.lock().unwrap().clear();
    app.load_connect_cache(
        user,
        &fingerprint,
        &remote,
        Refresh::Recent,
        &uninterrupted(),
    )
    .await
    .unwrap();
    assert_eq!(*remote.pages.lock().unwrap(), [0, 20]);
    assert_eq!(remote.downloads.lock().unwrap().len(), 23);
    let switched = cached_account(&app, user, &fingerprint).await;
    assert_ne!(original.source, switched.source);
}
