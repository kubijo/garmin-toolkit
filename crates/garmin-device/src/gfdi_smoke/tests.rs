use std::fs;
use std::future::pending;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::sync::Arc;

use tokio::sync::Notify;

use super::*;

#[tokio::test]
async fn file_access_roundtrip_retains_original_bytes_and_closes_only_owned_handles() {
    for mode in [LinkMode::Plain, LinkMode::Reliable] {
        let parent = tempfile::tempdir().unwrap();
        let mut trace = Capture::create_download(&parent.path().join("capture")).unwrap();
        let mut peer = download_fake::FilePeer::new().unwrap();
        let expected = peer.source().to_vec();
        let (identity, file) = fetch::execute_download(
            &mut peer,
            &mut trace,
            Duration::from_secs(5),
            mode,
            pending(),
        )
        .await
        .unwrap();
        assert_eq!(identity.model, "Demo tracker");
        assert_eq!(file.bytes, expected);
        assert!(peer.disconnected);
        let registrations: Vec<_> = peer
            .writes
            .iter()
            .filter(|bytes| bytes.len() == 13 && bytes[..2] == [0, 0])
            .collect();
        assert_eq!(registrations.len(), 2);
        assert_eq!(registrations[0][10..12], [1, 0]);
        assert_eq!(registrations[1][10..12], [0x18, 0x20]);
        assert_eq!(registrations[0][2..10], registrations[1][2..10]);
        let closes: Vec<_> = peer
            .writes
            .iter()
            .filter(|bytes| bytes.len() == 13 && bytes[..2] == [0, 2])
            .collect();
        assert_eq!(closes.len(), 1); // The peer already closed the transfer pipe.
        assert_eq!(closes[0][2..12], registrations[0][2..12]);
        assert_eq!(closes[0][12], 0x86);
        assert_eq!(
            peer.writes
                .iter()
                .filter(|bytes| bytes.first() == Some(&0))
                .count(),
            3
        );
    }
}

#[tokio::test]
async fn synthetic_download_command_retains_a_valid_private_fit() {
    use sha2::Digest as _;
    let parent = tempfile::tempdir().unwrap();
    let directory = parent.path().join("download");
    let report = simulate_download(directory.clone()).await.unwrap();
    let original = download_fake::FilePeer::new().unwrap().source().to_vec();
    assert_eq!(fs::read(&report.path).unwrap(), original);
    assert_eq!(report.bytes, original.len());
    assert_eq!(report.sha256, hex::encode(sha2::Sha256::digest(&original)));
    assert_eq!(
        fs::metadata(&report.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(simulate_download(directory).await.is_err());
}

#[tokio::test]
async fn cancelled_download_before_connect_does_not_touch_device() {
    let parent = tempfile::tempdir().unwrap();
    let mut trace = Capture::create_download(&parent.path().join("capture")).unwrap();
    let mut peer = download_fake::FilePeer::new().unwrap();
    assert!(matches!(
        fetch::execute_download(
            &mut peer,
            &mut trace,
            Duration::from_secs(5),
            LinkMode::Reliable,
            async { Ok(()) }
        )
        .await,
        Err(SmokeError::Cancelled)
    ));
    assert!(!peer.disconnected);
    assert_eq!(peer.writes, Vec::<Vec<u8>>::new());
}

#[tokio::test(start_paused = true)]
async fn download_faults_and_timeouts_still_disconnect_owned_attempts() {
    for fault in [
        Fault::Connect,
        Fault::Inventory,
        Fault::Subscribe,
        Fault::Register,
        Fault::Receive,
        Fault::NoRoute,
        Fault::Hold,
    ] {
        let parent = tempfile::tempdir().unwrap();
        let mut trace = Capture::create_download(&parent.path().join("capture")).unwrap();
        let mut peer = Peer::new(fault);
        assert!(
            fetch::execute_download(
                &mut peer,
                &mut trace,
                Duration::from_secs(1),
                LinkMode::Reliable,
                pending()
            )
            .await
            .is_err()
        );
        assert!(peer.inner.disconnected);
    }
}

#[derive(Clone, Copy)]
enum FileFault {
    PipeReceive,
    PipeRegister,
    Close,
    Disconnect,
}

struct FileFaultPeer {
    inner: download_fake::FilePeer,
    fault: FileFault,
    injected: bool,
}

#[async_trait]
impl Transport for FileFaultPeer {
    async fn connect(&mut self) -> Result<(), SmokeError> {
        self.inner.connect().await
    }
    async fn inventory(&mut self) -> Result<Vec<garmin_gfdi::Characteristic>, SmokeError> {
        self.inner.inventory().await
    }
    async fn subscribe(&mut self, route: garmin_gfdi::Route) -> Result<(), SmokeError> {
        self.inner.subscribe(route).await
    }
    async fn write(&mut self, bytes: &[u8]) -> Result<(), SmokeError> {
        if bytes.len() == 13
            && ((matches!(self.fault, FileFault::Close) && bytes[..2] == [0, 2])
                || (matches!(self.fault, FileFault::PipeRegister)
                    && bytes[..2] == [0, 0]
                    && bytes[10..12] == [0x18, 0x20]))
        {
            return Err(failed());
        }
        self.inner.write(bytes).await
    }
    async fn next(&mut self) -> Result<Option<Vec<u8>>, SmokeError> {
        let bytes = self.inner.next().await?;
        if matches!(self.fault, FileFault::PipeReceive)
            && !self.injected
            && bytes
                .as_ref()
                .is_some_and(|bytes| bytes[0] & 0x80 != 0 && (bytes[0] >> 4) & 7 == 2)
        {
            self.injected = true;
            return Err(failed());
        }
        Ok(bytes)
    }
    async fn disconnect(&mut self) -> Result<(), SmokeError> {
        self.inner.disconnect().await?;
        if matches!(self.fault, FileFault::Disconnect) {
            return Err(failed());
        }
        Ok(())
    }
}

#[tokio::test]
async fn partial_file_and_cleanup_faults_do_not_report_success_or_leak_connections() {
    for fault in [
        FileFault::PipeReceive,
        FileFault::PipeRegister,
        FileFault::Close,
        FileFault::Disconnect,
    ] {
        let parent = tempfile::tempdir().unwrap();
        let mut trace = Capture::create_download(&parent.path().join("capture")).unwrap();
        let mut peer = FileFaultPeer {
            inner: download_fake::FilePeer::new().unwrap(),
            fault,
            injected: false,
        };
        let result = fetch::execute_download(
            &mut peer,
            &mut trace,
            Duration::from_secs(5),
            LinkMode::Reliable,
            pending(),
        )
        .await;
        assert!(result.is_err());
        assert!(peer.inner.disconnected);
        if matches!(fault, FileFault::PipeReceive) {
            let closes: Vec<_> = peer
                .inner
                .writes
                .iter()
                .filter(|bytes| bytes.len() == 13 && bytes[..2] == [0, 2])
                .collect();
            assert_eq!(closes.len(), 2);
            assert_eq!(closes[0][10..13], [0x18, 0x20, 0x82]);
            assert_eq!(closes[1][10..13], [1, 0, 0x86]);
        }
    }
}

#[tokio::test]
async fn download_capture_exhaustion_still_releases_owned_resources() {
    let parent = tempfile::tempdir().unwrap();
    let mut trace = capture(&parent);
    for _ in 0..4090 {
        trace.record("fill", None).unwrap();
    }
    let mut peer = download_fake::FilePeer::new().unwrap();
    assert!(
        fetch::execute_download(
            &mut peer,
            &mut trace,
            Duration::from_secs(5),
            LinkMode::Reliable,
            pending()
        )
        .await
        .is_err()
    );
    assert!(peer.disconnected);
    assert!(
        peer.writes
            .iter()
            .any(|bytes| bytes.len() == 13 && bytes[..2] == [0, 2])
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    Connect,
    Inventory,
    Subscribe,
    Register,
    Ack,
    Receive,
    Close,
    Disconnect,
    NoRoute,
    Hold,
    Flood,
}

struct Peer {
    inner: fake::SyntheticPeer,
    fault: Fault,
    closing: bool,
    reads: usize,
    registered: Arc<Notify>,
}

impl Peer {
    fn new(fault: Fault) -> Self {
        Self {
            inner: fake::SyntheticPeer::new(),
            fault,
            closing: false,
            reads: 0,
            registered: Arc::new(Notify::new()),
        }
    }
}

fn failed() -> SmokeError {
    SmokeError::Preconditions("injected test failure")
}

#[async_trait]
impl Transport for Peer {
    async fn connect(&mut self) -> Result<(), SmokeError> {
        if self.fault == Fault::Connect {
            return Err(failed());
        }
        self.inner.connect().await
    }

    async fn inventory(&mut self) -> Result<Vec<Characteristic>, SmokeError> {
        if self.fault == Fault::Inventory {
            return Err(failed());
        }
        if self.fault == Fault::NoRoute {
            return Ok(Vec::new());
        }
        self.inner.inventory().await
    }

    async fn subscribe(&mut self, route: Route) -> Result<(), SmokeError> {
        if self.fault == Fault::Subscribe {
            return Err(failed());
        }
        self.inner.subscribe(route).await
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<(), SmokeError> {
        if bytes.len() == 13 && bytes[..2] == [0, 2] {
            self.closing = true;
            if self.fault == Fault::Close {
                return Err(failed());
            }
        }
        if (bytes.len() == 13 && bytes[..2] == [0, 0] && self.fault == Fault::Register)
            || (bytes.len() == 2 && self.fault == Fault::Ack)
        {
            return Err(failed());
        }
        self.inner.write(bytes).await
    }

    async fn next(&mut self) -> Result<Option<Vec<u8>>, SmokeError> {
        self.reads += 1;
        if !self.closing && self.reads > 1 {
            self.registered.notify_one();
            match self.fault {
                Fault::Hold => return pending().await,
                Fault::Flood => return Ok(Some(vec![0xe0, 0])),
                Fault::Receive => return Err(failed()),
                _ => {}
            }
        }
        self.inner.next().await
    }

    async fn disconnect(&mut self) -> Result<(), SmokeError> {
        self.inner.disconnect().await?;
        if self.fault == Fault::Disconnect {
            return Err(failed());
        }
        Ok(())
    }
}

fn capture(parent: &tempfile::TempDir) -> Capture {
    Capture::create(&parent.path().join("capture")).unwrap()
}

#[tokio::test]
async fn synthetic_driver_observes_identity_and_only_closes_its_own_handle() {
    for mode in [LinkMode::Plain, LinkMode::Reliable] {
        let parent = tempfile::tempdir().unwrap();
        let mut trace = capture(&parent);
        let mut peer = Peer::new(Fault::None);
        let identity = execute(
            &mut peer,
            &mut trace,
            Duration::from_secs(1),
            mode,
            pending(),
        )
        .await
        .unwrap();
        assert_eq!(identity.model, "Demo tracker");
        assert!(peer.inner.disconnected);
        let writes = &peer.inner.writes;
        assert_eq!(&writes[0][..2], &[0, 0]);
        assert_eq!(&writes.last().unwrap()[..2], &[0, 2]);
        assert_eq!(writes[0][2..12], writes.last().unwrap()[2..12]);
        assert!(
            writes
                .iter()
                .all(|bytes| bytes.len() == 2 || bytes.len() == 13)
        );
        assert_eq!(writes.iter().filter(|bytes| bytes.len() == 13).count(), 2);
        let directory = parent.path().join("capture");
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(directory.join("events.jsonl"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        for line in fs::read_to_string(directory.join("events.jsonl"))
            .unwrap()
            .lines()
        {
            let event: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(event["elapsed_ms"].is_number());
        }
    }
}

#[tokio::test(start_paused = true)]
async fn every_observation_failure_still_disconnects() {
    for fault in [
        Fault::Connect,
        Fault::Inventory,
        Fault::Subscribe,
        Fault::Register,
        Fault::Ack,
        Fault::Receive,
        Fault::NoRoute,
        Fault::Hold,
    ] {
        let parent = tempfile::tempdir().unwrap();
        let mut trace = capture(&parent);
        let mut peer = Peer::new(fault);
        assert!(
            execute(
                &mut peer,
                &mut trace,
                Duration::from_secs(1),
                LinkMode::Reliable,
                pending()
            )
            .await
            .is_err()
        );
        assert!(peer.inner.disconnected);
        if matches!(fault, Fault::Ack | Fault::Receive | Fault::Hold) {
            assert!(peer.closing);
        }
    }
}

#[tokio::test]
async fn cleanup_failure_cannot_be_reported_as_a_successful_smoke() {
    for fault in [Fault::Close, Fault::Disconnect] {
        let parent = tempfile::tempdir().unwrap();
        let mut trace = capture(&parent);
        let mut peer = Peer::new(fault);
        assert!(matches!(
            execute(
                &mut peer,
                &mut trace,
                Duration::from_secs(1),
                LinkMode::Reliable,
                pending()
            )
            .await,
            Err(SmokeError::Cleanup { .. })
        ));
        assert!(peer.inner.disconnected);
    }
}

#[tokio::test]
async fn cancellation_before_connect_does_not_disconnect_any_device() {
    let parent = tempfile::tempdir().unwrap();
    let mut trace = capture(&parent);
    let mut peer = Peer::new(Fault::None);
    assert!(matches!(
        execute(
            &mut peer,
            &mut trace,
            Duration::from_secs(1),
            LinkMode::Reliable,
            async { Ok(()) }
        )
        .await,
        Err(SmokeError::Cancelled)
    ));
    assert!(!peer.inner.disconnected);
    assert_eq!(peer.inner.writes, Vec::<Vec<u8>>::new());
}

#[tokio::test]
async fn signal_setup_failure_stops_before_connect() {
    let parent = tempfile::tempdir().unwrap();
    let mut trace = capture(&parent);
    let mut peer = Peer::new(Fault::None);
    assert!(matches!(
        execute(
            &mut peer,
            &mut trace,
            Duration::from_secs(1),
            LinkMode::Reliable,
            async { Err(failed()) }
        )
        .await,
        Err(SmokeError::Preconditions(_))
    ));
    assert!(!peer.inner.disconnected);
    assert_eq!(peer.inner.writes, Vec::<Vec<u8>>::new());
}

#[tokio::test]
async fn cancellation_after_registration_closes_the_known_handle_and_disconnects() {
    let parent = tempfile::tempdir().unwrap();
    let mut trace = capture(&parent);
    let mut peer = Peer::new(Fault::Hold);
    let notification = peer.registered.clone();
    assert!(matches!(
        execute(
            &mut peer,
            &mut trace,
            Duration::from_secs(1),
            LinkMode::Reliable,
            async {
                notification.notified().await;
                Ok(())
            }
        )
        .await,
        Err(SmokeError::Cancelled)
    ));
    assert!(peer.closing);
    assert!(peer.inner.disconnected);
}

#[tokio::test]
async fn trace_exhaustion_does_not_prevent_owned_cleanup() {
    let parent = tempfile::tempdir().unwrap();
    let mut trace = capture(&parent);
    let mut peer = Peer::new(Fault::Flood);
    assert!(
        execute(
            &mut peer,
            &mut trace,
            Duration::from_secs(10),
            LinkMode::Reliable,
            pending()
        )
        .await
        .is_err()
    );
    assert!(peer.closing);
    assert!(peer.inner.disconnected);
}

#[tokio::test]
async fn close_confirmation_capture_failure_is_reported_after_releasing_handle() {
    let parent = tempfile::tempdir().unwrap();
    let mut trace = capture(&parent);
    for _ in 0..4095 {
        trace.record("fill", None).unwrap();
    }
    let mut peer = fake::SyntheticPeer::new();
    peer.write(&[0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 2])
        .await
        .unwrap();
    let close = [0, 2, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0x86];
    assert!(matches!(
        close_owned(&mut peer, &mut trace, close).await,
        Err(SmokeError::TraceLimit)
    ));
    assert_eq!(peer.writes.last().unwrap(), &close);
    assert_eq!(peer.next().await.unwrap(), None);
}

#[test]
fn capture_refuses_existing_paths_and_symlinks_without_overwriting() {
    let parent = tempfile::tempdir().unwrap();
    let existing = parent.path().join("existing");
    fs::create_dir(&existing).unwrap();
    fs::write(existing.join("events.jsonl"), b"keep").unwrap();
    assert!(Capture::create(&existing).is_err());
    let link = parent.path().join("link");
    symlink(&existing, &link).unwrap();
    assert!(Capture::create(&link).is_err());
    assert_eq!(fs::read(existing.join("events.jsonl")).unwrap(), b"keep");
    assert!(validate_timeout(Duration::ZERO).is_err());
    assert!(validate_timeout(Duration::from_secs(61)).is_err());
    assert!(validate_timeout(Duration::from_secs(1)).is_ok());
    assert!(validate_timeout(Duration::from_secs(60)).is_ok());
}
