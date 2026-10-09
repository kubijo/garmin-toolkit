use std::os::unix::fs::PermissionsExt as _;

use tempfile::tempdir;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn timeout_kills_and_reaps_the_worker_and_releases_capacity() -> TestResult {
    let _test = TEST_LOCK.lock().await;
    let root = tempdir()?;
    let executable = root.path().join("worker");
    let pid_file = root.path().join("pid");
    std::fs::write(
        &executable,
        format!(
            "#!/bin/sh\necho $$ > '{}'\nexec sleep 30\n",
            pid_file.display()
        ),
    )?;
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))?;
    let parser = Parser {
        executable,
        timeout: Duration::from_millis(100),
    };
    let result = parser.parse(Arc::from([])).await;
    assert!(matches!(result, Err(Error::Timeout)), "{result:?}");
    let pid: u32 = std::fs::read_to_string(pid_file)?.trim().parse()?;
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    assert_eq!(PARSER_SLOT.available_permits(), 1);
    Ok(())
}

#[tokio::test]
async fn busy_capacity_rejects_work_before_starting_a_process() -> TestResult {
    let _test = TEST_LOCK.lock().await;
    let _slot = PARSER_SLOT.acquire().await?;
    let parser = Parser::new("/missing/worker")?;
    assert!(matches!(
        parser.parse(Arc::from([])).await,
        Err(Error::Busy)
    ));
    Ok(())
}

#[tokio::test]
async fn missing_worker_explains_the_failed_import_and_retains_the_io_cause() -> TestResult {
    let _test = TEST_LOCK.lock().await;
    let root = tempdir()?;
    let parser = Parser::new(root.path().join("garmin-gpx-worker"))?;
    let error = parser
        .parse(Arc::from([]))
        .await
        .expect_err("worker is absent");
    assert!(
        error
            .to_string()
            .contains("garmin-gpx-worker executable was not found")
    );
    assert!(
        matches!(error, Error::WorkerMissing(ref source) if source.kind() == std::io::ErrorKind::NotFound)
    );
    assert_eq!(PARSER_SLOT.available_permits(), 1);
    Ok(())
}

#[tokio::test]
async fn oversized_output_is_rejected_and_the_worker_is_reaped() -> TestResult {
    let _test = TEST_LOCK.lock().await;
    let root = tempdir()?;
    let executable = root.path().join("worker");
    std::fs::write(&executable, "#!/bin/sh\nexec head -c 33554433 /dev/zero\n")?;
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))?;
    let parser = Parser::new(&executable)?;
    assert!(matches!(
        parser.parse(Arc::from([])).await,
        Err(Error::ReplyTooLarge)
    ));
    assert_eq!(PARSER_SLOT.available_permits(), 1);
    Ok(())
}
