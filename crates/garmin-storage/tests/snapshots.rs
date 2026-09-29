//! Portable snapshot validation and atomic publication.

use std::io;
use std::{
    error::Error,
    fs,
    io::{Cursor, Read},
};

use futures_lite::future::block_on;
use garmin_model::identity::{Profile, Role, User, UserId};
use garmin_storage::{
    Storage,
    snapshot::{Limits, PreparedRestore},
};
use sha2::{Digest as _, Sha256};
use sqlx::Connection as _;
use tempfile::{TempDir, tempdir};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

async fn fixture() -> Result<(TempDir, Vec<u8>, User)> {
    let root = tempdir()?;
    let mut storage = Storage::open(root.path().join("source.sqlite3")).await?;
    let user = User::from_parts(
        UserId::from_u128(42),
        Role::Owner,
        Profile::from_display_name("Snapshot Rider".parse()?),
    );
    storage.save_user(&user).await?;
    let output = root.path().join("snapshot.tar.zst");
    storage.snapshot(&output, Limits::default()).await?;
    storage.close().await;
    Ok((root, fs::read(output)?, user))
}

fn members(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut archive = tar::Archive::new(zstd::stream::read::Decoder::new(bytes)?);
    archive
        .entries()?
        .map(|entry| {
            let mut entry = entry?;
            let name = entry.path()?.to_string_lossy().into_owned();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            Ok((name, bytes))
        })
        .collect()
}

fn tar_bytes(members: &[(String, Vec<u8>)], terminated: bool) -> Result<Vec<u8>> {
    let mut archive = tar::Builder::new(Vec::new());
    for (name, bytes) in members {
        let mut header = tar::Header::new_ustar();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        archive.append_data(&mut header, name, bytes.as_slice())?;
    }
    if terminated {
        Ok(archive.into_inner()?)
    } else {
        Ok(archive.get_ref().clone())
    }
}

fn compress(bytes: &[u8], checksum: bool) -> Result<Vec<u8>> {
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 3)?;
    encoder.include_checksum(checksum)?;
    io::copy(&mut Cursor::new(bytes), &mut encoder)?;
    Ok(encoder.finish()?)
}

#[test]
fn restores_known_older_schema_and_defaults_new_profile_preferences() -> Result {
    block_on(async {
        let (root, bytes, user) = fixture().await?;
        let path = root.path().join("legacy.sqlite3");
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let mut connection = sqlx::SqliteConnection::connect_with(&options).await?;
        sqlx::migrate!("./migrations")
            .run_to(1, &mut connection)
            .await?;
        let id = user.id().to_string();
        sqlx::query_file!("tests/queries/insert-legacy-user.sql", id)
            .execute(&mut connection)
            .await?;
        connection.close().await?;
        let database = fs::read(&path)?;
        let mut items = members(&bytes)?;
        let mut manifest: garmin_storage::snapshot::Manifest = serde_json::from_slice(&items[0].1)?;
        manifest.schema.truncate(1);
        manifest.database_bytes = u64::try_from(database.len())?;
        manifest.database_sha256 = hex::encode(Sha256::digest(&database));
        items[0].1 = serde_json::to_vec(&manifest)?;
        items[1].1 = database;
        let archive = compress(&tar_bytes(&items, true)?, true)?;
        let destination = root.path().join("restored.sqlite3");
        PreparedRestore::read(archive.as_slice(), root.path(), Limits::default())
            .await?
            .publish(&destination)?;
        let restored = Storage::open(&destination).await?;
        assert_eq!(restored.user(user.id()).await?, Some(user));
        restored.check_integrity().await?;
        restored.close().await;
        Ok(())
    })
}

#[test]
fn snapshot_round_trip_preserves_profiles_and_publishes_without_replacement() -> Result {
    block_on(async {
        let (root, bytes, user) = fixture().await?;
        let destination = root.path().join("restored.sqlite3");
        let prepared =
            PreparedRestore::read(bytes.as_slice(), root.path(), Limits::default()).await?;
        assert!(!destination.exists());
        prepared.publish(&destination)?;
        let restored = Storage::open(&destination).await?;
        assert_eq!(restored.users().await?, [user]);
        restored.check_integrity().await?;
        restored.close().await;
        let before = fs::read(&destination)?;
        let prepared =
            PreparedRestore::read(bytes.as_slice(), root.path(), Limits::default()).await?;
        assert!(prepared.publish(&destination).is_err());
        assert_eq!(fs::read(&destination)?, before);
        Ok(())
    })
}

#[test]
fn dropping_restore_cleans_staging_and_preserves_source() -> Result {
    block_on(async {
        let (root, bytes, _) = fixture().await?;
        let before = fs::read(root.path().join("source.sqlite3"))?;
        let count = fs::read_dir(root.path())?.count();
        let prepared =
            PreparedRestore::read(bytes.as_slice(), root.path(), Limits::default()).await?;
        assert!(fs::read_dir(root.path())?.count() > count);
        drop(prepared);
        assert_eq!(fs::read_dir(root.path())?.count(), count);
        assert_eq!(fs::read(root.path().join("source.sqlite3"))?, before);
        Ok(())
    })
}

#[test]
fn rejects_corrupt_truncated_and_multiple_zstandard_frames() -> Result {
    block_on(async {
        let (root, bytes, _) = fixture().await?;
        for length in [0, 1, bytes.len() / 2, bytes.len() - 1] {
            assert!(
                PreparedRestore::read(&bytes[..length], root.path(), Limits::default())
                    .await
                    .is_err(),
                "accepted truncation at {length}"
            );
        }
        let mut damaged = bytes.clone();
        *damaged.last_mut().ok_or("empty snapshot")? ^= 1;
        let mut multiple = bytes.clone();
        multiple.extend_from_slice(&bytes);
        let mut trailing = bytes;
        trailing.push(1);
        for input in [damaged, multiple, trailing] {
            assert!(
                PreparedRestore::read(input.as_slice(), root.path(), Limits::default())
                    .await
                    .is_err()
            );
        }
        Ok(())
    })
}

#[test]
fn rejects_missing_duplicate_reordered_and_unexpected_entries() -> Result {
    block_on(async {
        let (root, bytes, _) = fixture().await?;
        let original = members(&bytes)?;
        let mut duplicate = original.clone();
        duplicate.push(original[0].clone());
        let mut unexpected = original.clone();
        unexpected.push(("other".into(), vec![]));
        let mut reordered = original.clone();
        reordered.reverse();
        for items in [
            vec![],
            original[..1].to_vec(),
            duplicate,
            unexpected,
            reordered,
        ] {
            let bytes = compress(&tar_bytes(&items, true)?, true)?;
            assert!(
                PreparedRestore::read(bytes.as_slice(), root.path(), Limits::default())
                    .await
                    .is_err()
            );
        }
        let mut appended = tar_bytes(&original, true)?;
        appended.extend(tar_bytes(&original, true)?);
        let bytes = compress(&appended, true)?;
        assert!(
            PreparedRestore::read(bytes.as_slice(), root.path(), Limits::default())
                .await
                .is_err()
        );
        Ok(())
    })
}

#[test]
fn rejects_incompatible_unknown_and_false_manifest_claims() -> Result {
    block_on(async {
        let (root, bytes, _) = fixture().await?;
        for (key, value) in [
            ("version", serde_json::json!(2)),
            ("schema", serde_json::json!([])),
            ("database_bytes", serde_json::json!(1)),
            ("database_sha256", serde_json::json!("incorrect")),
            ("unknown", serde_json::json!(true)),
        ] {
            let mut items = members(&bytes)?;
            let mut manifest: serde_json::Value = serde_json::from_slice(&items[0].1)?;
            manifest[key] = value;
            items[0].1 = serde_json::to_vec(&manifest)?;
            let changed = compress(&tar_bytes(&items, true)?, true)?;
            assert!(
                PreparedRestore::read(changed.as_slice(), root.path(), Limits::default())
                    .await
                    .is_err(),
                "accepted {key}"
            );
        }
        Ok(())
    })
}

#[test]
fn bounds_compressed_input_database_and_manifest() -> Result {
    block_on(async {
        let (root, bytes, _) = fixture().await?;
        for limits in [
            Limits {
                compressed_bytes: 32,
                ..Limits::default()
            },
            Limits {
                database_bytes: 1024,
                compressed_bytes: 1024,
            },
        ] {
            assert!(
                PreparedRestore::read(bytes.as_slice(), root.path(), limits)
                    .await
                    .is_err()
            );
        }
        let mut items = members(&bytes)?;
        items[0].1 = vec![b' '; 32 * 1024];
        let oversized = compress(&tar_bytes(&items, true)?, true)?;
        assert!(
            PreparedRestore::read(oversized.as_slice(), root.path(), Limits::default())
                .await
                .is_err()
        );
        Ok(())
    })
}

#[test]
fn library_accepts_optional_checksum_and_missing_tar_end_blocks() -> Result {
    block_on(async {
        let (root, bytes, _) = fixture().await?;
        let items = members(&bytes)?;
        // Explicit compatibility limits of the library APIs, not strict-format claims.
        for (checksum, terminated) in [(false, true), (true, false)] {
            let input = compress(&tar_bytes(&items, terminated)?, checksum)?;
            PreparedRestore::read(input.as_slice(), root.path(), Limits::default()).await?;
        }
        Ok(())
    })
}

struct Interrupted;
impl Read for Interrupted {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("transfer disconnected"))
    }
}

#[test]
fn interrupted_input_cleans_private_staging() -> Result {
    block_on(async {
        let root = tempdir()?;
        assert!(
            PreparedRestore::read(Interrupted, root.path(), Limits::default())
                .await
                .is_err()
        );
        assert_eq!(fs::read_dir(root.path())?.count(), 0);
        Ok(())
    })
}

#[test]
fn rejects_link_entries_without_extracting_them() -> Result {
    block_on(async {
        let root = tempdir()?;
        for kind in [
            tar::EntryType::Symlink,
            tar::EntryType::Link,
            tar::EntryType::Directory,
            tar::EntryType::Char,
        ] {
            let mut archive = tar::Builder::new(Vec::new());
            let mut header = tar::Header::new_ustar();
            header.set_entry_type(kind);
            header.set_size(0);
            header.set_mode(0o600);
            if kind.is_symlink() || kind.is_hard_link() {
                header.set_link_name("outside")?;
            }
            header.set_cksum();
            archive.append_data(&mut header, "manifest.json", io::empty())?;
            let bytes = compress(&archive.into_inner()?, true)?;
            assert!(
                PreparedRestore::read(bytes.as_slice(), root.path(), Limits::default())
                    .await
                    .is_err()
            );
            assert_eq!(fs::read_dir(root.path())?.count(), 0);
        }
        Ok(())
    })
}

#[test]
fn invalid_sqlite_is_rejected_even_with_matching_length_and_digest() -> Result {
    block_on(async {
        use sha2::{Digest, Sha256};
        let (root, bytes, _) = fixture().await?;
        let mut items = members(&bytes)?;
        items[1].1.fill(0);
        let mut manifest: serde_json::Value = serde_json::from_slice(&items[0].1)?;
        manifest["database_sha256"] = serde_json::json!(hex::encode(Sha256::digest(&items[1].1)));
        items[0].1 = serde_json::to_vec(&manifest)?;
        let invalid = compress(&tar_bytes(&items, true)?, true)?;
        assert!(
            PreparedRestore::read(invalid.as_slice(), root.path(), Limits::default())
                .await
                .is_err()
        );
        Ok(())
    })
}

#[test]
fn failed_snapshot_publication_preserves_existing_destination() -> Result {
    block_on(async {
        let root = tempdir()?;
        let mut storage = Storage::open(root.path().join("source.sqlite3")).await?;
        let output = root.path().join("snapshot.tar.zst");
        fs::write(&output, b"existing snapshot")?;
        assert!(storage.snapshot(&output, Limits::default()).await.is_err());
        assert_eq!(fs::read(&output)?, b"existing snapshot");
        storage.close().await;
        assert_eq!(fs::read_dir(root.path())?.count(), 2);
        Ok(())
    })
}

#[test]
fn restore_refuses_orphaned_sqlite_sidecars() -> Result {
    block_on(async {
        let (root, bytes, _) = fixture().await?;
        let destination = root.path().join("restored.sqlite3");
        for name in [
            "restored.sqlite3-wal",
            "restored.sqlite3-shm",
            "restored.sqlite3-journal",
        ] {
            let sidecar = root.path().join(name);
            fs::write(&sidecar, b"old store")?;
            let prepared =
                PreparedRestore::read(bytes.as_slice(), root.path(), Limits::default()).await?;
            assert!(prepared.publish(&destination).is_err());
            assert!(!destination.exists());
            assert_eq!(fs::read(&sidecar)?, b"old store");
            fs::remove_file(sidecar)?;
        }
        Ok(())
    })
}
