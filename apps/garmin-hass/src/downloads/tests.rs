use super::*;
use futures_util::FutureExt as _;
use std::sync::atomic::{AtomicUsize, Ordering};

fn prepared(size: u64, chunks: Vec<io::Result<Bytes>>) -> Prepared {
    Prepared::new(
        "test.bin".into(),
        size,
        "application/octet-stream",
        futures_util::stream::iter(chunks).boxed(),
    )
}

async fn cleanup_finished(downloads: &Downloads) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !downloads
            .reservations
            .lock()
            .expect("reservations")
            .is_empty()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cleanup completes");
}

#[tokio::test]
async fn single_use_response_keeps_file_until_body_finishes() -> anyhow::Result<()> {
    let downloads = Downloads::default();
    let owner = tempfile::NamedTempFile::new()?;
    let path = owner.path().to_owned();
    tokio::fs::write(&path, b"content").await?;
    let file = tokio::fs::File::open(&path).await?;
    let ticket = downloads
        .insert(Prepared::from_file("file.bin".into(), 7, file, owner))
        .map_err(anyhow::Error::msg)?;
    let response = downloads.take(ticket.token).expect("ticket").response();
    assert!(downloads.take(ticket.token).is_none());
    assert!(path.exists());
    assert_eq!(response.headers()[header::CONTENT_LENGTH], "7");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        response.headers()[header::X_CONTENT_TYPE_OPTIONS],
        "nosniff"
    );
    assert_eq!(
        axum::body::to_bytes(response.into_body(), 8)
            .await?
            .as_ref(),
        b"content"
    );
    assert!(!path.exists());
    assert!(
        downloads
            .reservations
            .lock()
            .expect("reservations")
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn expiry_and_abandoned_response_release_staged_files() -> anyhow::Result<()> {
    for expired in [true, false] {
        let downloads = Downloads::default();
        let owner = tempfile::NamedTempFile::new()?;
        let path = owner.path().to_owned();
        let file = tokio::fs::File::open(&path).await?;
        let ticket = downloads
            .insert(Prepared::from_file("file.bin".into(), 0, file, owner))
            .map_err(anyhow::Error::msg)?;
        if expired {
            downloads
                .pending
                .lock()
                .expect("pending")
                .get_mut(&ticket.token)
                .expect("ticket")
                .created = Instant::now()
                .checked_sub(TTL)
                .expect("expired ticket time");
            assert!(downloads.take(ticket.token).is_none());
        } else {
            let response = downloads.take(ticket.token).expect("ticket").response();
            assert!(path.exists());
            drop(response);
        }
        assert!(!path.exists());
        assert!(
            downloads
                .reservations
                .lock()
                .expect("reservations")
                .is_empty()
        );
    }
    Ok(())
}

#[tokio::test]
async fn active_downloads_keep_count_and_byte_budgets_reserved() {
    let downloads = Downloads::default();
    let mut active = Vec::new();
    for _ in 0..MAX_DOWNLOADS {
        let ticket = downloads.insert(prepared(0, vec![])).expect("capacity");
        active.push(downloads.take(ticket.token).expect("ticket"));
    }
    assert!(downloads.insert(prepared(0, vec![])).is_err());
    active.pop();
    assert!(downloads.insert(prepared(0, vec![])).is_ok());
    drop(active);

    let downloads = Downloads::default();
    let mut large = prepared(0, vec![]);
    large.staged_bytes = garmin_device::MAX_BROWSER_TRANSFER_BYTES;
    let ticket = downloads.insert(large).expect("byte budget");
    let active = downloads.take(ticket.token).expect("ticket");
    let mut extra = prepared(0, vec![]);
    extra.staged_bytes = 1;
    assert!(downloads.insert(extra).is_err());
    drop(active);
    let mut overflow = prepared(0, vec![]);
    overflow.staged_bytes = u64::MAX;
    assert!(downloads.insert(overflow).is_err());
}

#[tokio::test]
async fn duplicate_operation_does_not_cancel_original_and_drop_cleans_once() {
    for abandon_registry in [false, true] {
        let downloads = Downloads::default();
        let cleaned = Arc::new(AtomicUsize::new(0));
        let operation = Uuid::new_v4();
        let source = || {
            let cleaned = Arc::clone(&cleaned);
            prepared(1, vec![Ok(Bytes::from_static(b"x"))]).with_operation(
                operation,
                async move {
                    cleaned.fetch_add(1, Ordering::SeqCst);
                }
                .boxed(),
            )
        };
        let ticket = downloads.insert(source()).expect("ticket");
        assert!(downloads.insert(source()).is_err());
        assert_eq!(cleaned.load(Ordering::SeqCst), 0);
        if abandon_registry {
            let reservations = Arc::clone(&downloads.reservations);
            drop(downloads);
            tokio::time::timeout(Duration::from_secs(2), async {
                while !reservations.lock().expect("reservations").is_empty() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("registry cleanup");
        } else {
            let response = downloads.take(ticket.token).expect("ticket").response();
            assert!(downloads.insert(source()).is_err());
            drop(response);
            cleanup_finished(&downloads).await;
        }
        assert_eq!(cleaned.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn invalid_streams_fail_without_leaking_reservations() {
    for chunks in [
        vec![],
        vec![Ok(Bytes::new())],
        vec![Ok(Bytes::from_static(b"too long"))],
        vec![Ok(Bytes::from(vec![0; CHUNK_BYTES + 1]))],
        vec![Err(io::Error::other("source failed"))],
    ] {
        let downloads = Downloads::default();
        let ticket = downloads.insert(prepared(1, chunks)).expect("ticket");
        let response = downloads.take(ticket.token).expect("ticket").response();
        assert!(
            axum::body::to_bytes(response.into_body(), CHUNK_BYTES * 2)
                .await
                .is_err()
        );
        assert!(
            downloads
                .reservations
                .lock()
                .expect("reservations")
                .is_empty()
        );
    }
}
