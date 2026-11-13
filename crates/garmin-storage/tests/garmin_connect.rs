use std::{error::Error, io::Read};

use futures_lite::future::block_on;
use garmin_model::{
    garmin_connect::PersonalApiKey,
    identity::{Profile, Role, User, UserId},
};
use garmin_storage::{
    Storage,
    snapshot::{Limits, PreparedRestore},
};

type TestResult = Result<(), Box<dyn Error>>;

fn user(id: u128) -> Result<User, Box<dyn Error>> {
    Ok(User::from_parts(
        UserId::from_u128(id),
        Role::Owner,
        Profile::from_display_name("Rider".parse()?),
    ))
}

#[test]
fn keys_survive_restart_remain_profile_scoped_and_can_be_replaced_and_removed() -> TestResult {
    block_on(async {
        let root = tempfile::tempdir()?;
        let path = root.path().join("storage.sqlite3");
        let storage = Storage::open(&path).await?;
        let first = user(1)?;
        let second = user(2)?;
        storage.save_user(&first).await?;
        storage.save_user(&second).await?;
        let key = PersonalApiKey::from_string("first-test-key".into())?;
        storage
            .set_garmin_connect_key(first.id(), Some(&key))
            .await?;
        storage.close().await;
        let storage = Storage::open(&path).await?;
        assert_eq!(
            storage
                .garmin_connect_key(first.id())
                .await?
                .unwrap()
                .expose(),
            key.expose()
        );
        assert!(!storage.garmin_connect_configured(second.id()).await?);
        assert!(storage.garmin_connect_key(second.id()).await?.is_none());
        let replacement = PersonalApiKey::from_string("replacement-test-key".into())?;
        storage
            .set_garmin_connect_key(first.id(), Some(&replacement))
            .await?;
        assert_eq!(
            storage
                .garmin_connect_key(first.id())
                .await?
                .unwrap()
                .expose(),
            replacement.expose()
        );
        storage.set_garmin_connect_key(first.id(), None).await?;
        assert!(!storage.garmin_connect_configured(first.id()).await?);
        assert!(storage.garmin_connect_key(first.id()).await?.is_none());
        Ok(())
    })
}

#[test]
fn backups_contain_no_key_bytes_and_do_not_delete_live_credentials() -> TestResult {
    block_on(async {
        let root = tempfile::tempdir()?;
        let mut storage = Storage::open(root.path().join("live.sqlite3")).await?;
        let user = user(1)?;
        storage.save_user(&user).await?;
        let previous = PersonalApiKey::from_string("previous-snapshot-secret-0987654321".into())?;
        storage
            .set_garmin_connect_key(user.id(), Some(&previous))
            .await?;
        let key = PersonalApiKey::from_string("unique-snapshot-secret-1234567890".into())?;
        storage
            .set_garmin_connect_key(user.id(), Some(&key))
            .await?;
        let snapshot = root.path().join("backup.tar.zst");
        storage.snapshot(&snapshot, Limits::default()).await?;
        assert!(storage.garmin_connect_configured(user.id()).await?);
        assert_eq!(
            storage
                .garmin_connect_key(user.id())
                .await?
                .unwrap()
                .expose(),
            key.expose()
        );
        let mut unpacked = Vec::new();
        zstd::stream::read::Decoder::new(std::fs::File::open(&snapshot)?)?
            .read_to_end(&mut unpacked)?;
        for secret in [&key, &previous] {
            assert!(
                !unpacked
                    .windows(secret.expose().len())
                    .any(|bytes| bytes == secret.expose().as_bytes())
            );
        }
        storage.set_garmin_connect_key(user.id(), None).await?;
        let removed = root.path().join("removed-key.tar.zst");
        storage.snapshot(&removed, Limits::default()).await?;
        unpacked.clear();
        zstd::stream::read::Decoder::new(std::fs::File::open(removed)?)?
            .read_to_end(&mut unpacked)?;
        for secret in [&key, &previous] {
            assert!(
                !unpacked
                    .windows(secret.expose().len())
                    .any(|bytes| bytes == secret.expose().as_bytes())
            );
        }
        let restored = root.path().join("restored.sqlite3");
        PreparedRestore::read(
            std::fs::File::open(snapshot)?,
            root.path(),
            Limits::default(),
        )
        .await?
        .publish(&restored)?;
        let restored = Storage::open(restored).await?;
        assert!(!restored.garmin_connect_configured(user.id()).await?);
        assert_eq!(restored.users().await?.len(), 1);
        Ok(())
    })
}
