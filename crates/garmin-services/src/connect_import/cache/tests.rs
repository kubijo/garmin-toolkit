use std::sync::Mutex;

use super::*;
use garmin_model::{
    activity::ActivitySport,
    garmin_connect::{ActivityRequest, RemoteActivity},
    identity::{Profile, Role, User, UserId},
};

struct MockRemote {
    facts: AccountFacts,
    summaries: Vec<CachedActivity>,
    archive: Vec<u8>,
    fail: Option<(i64, u16)>,
    pages: Mutex<Vec<u32>>,
    downloads: Mutex<Vec<i64>>,
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
                        distance_metres: Some(1000.0),
                        duration_seconds: Some(60.0),
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
    app.load_connect_cache(user, &fingerprint, &remote, &|_| {})
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
    app.load_connect_cache(user, &fingerprint, &remote, &|_| {})
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
    app.load_connect_cache(user, &fingerprint, &remote, &report)
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
    app.load_connect_cache(user, &fingerprint, &remote, &report)
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
        app.load_connect_cache(user, &fingerprint, &remote, &|_| {})
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
    app.load_connect_cache(user, &fingerprint, &remote, &|_| {})
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
async fn failed_metadata_refresh_preserves_the_previous_snapshot_and_account_switch_cannot_reuse_its_files()
 {
    let root = tempfile::tempdir().unwrap();
    let (app, user, key) = application(&root.path().join("cache.sqlite3")).await;
    let mut remote = MockRemote::new();
    let fingerprint = credential_hash(&key);
    app.load_connect_cache(user, &fingerprint, &remote, &|_| {})
        .await
        .unwrap();
    let previous = cached_account(&app, user, &fingerprint).await;
    remote.summaries[20] = remote.summaries[0].clone();
    assert!(
        app.load_connect_cache(user, &fingerprint, &remote, &|_| {})
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
    app.load_connect_cache(user, "replacement-key", &remote, &|_| {})
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
