//! Explicit workout delivery and cleanup of the retained synthetic Course only.

use super::*;
use garmin_device::{
    DeviceManifest, DevicePathState, MountedMtpUploadProgress, storage::DeviceWrite,
};
use garmin_fit::fixture::{integrity::Header, workout};

#[tokio::test]
async fn workout_delivery_and_course_cleanup_use_verified_bytes() -> Result<()> {
    let root = tempfile::tempdir()?;
    let watch = Device::open(root.path().join("watch"))?;
    let connector: Arc<dyn Connector> = Arc::new(DirectoryConnector {
        root: watch.root().to_owned(),
        storage_id: fixture::STORAGE_ID.to_owned(),
        storage_label: fixture::STORAGE_LABEL.to_owned(),
    });
    let host = root.path().join("host");
    recover_course(&host, fixture::KEY, Arc::clone(&connector)).await?;
    let transfer = fs::read_dir(host.join("course-transfers"))?
        .next()
        .context("missing transfer")??
        .path();
    let intent = read_intent(&transfer)?;
    let payload = fs::read(transfer.join("payload.fit"))?;
    let unrelated = watch.root().join("Garmin/Courses/unrelated.fit");
    let unrelated_bytes = rewritten_course(&payload, "Keep me")?;
    fs::write(&unrelated, &unrelated_bytes)?;
    let renamed = watch.root().join("Garmin/Courses/TK_Recovery.fit");
    fs::write(&renamed, rewritten_course(&payload, "TK Recovery")?)?;
    fs::remove_file(watch.root().join(intent.path.as_path()))?;
    let connection = connector.connect().await?;
    cleanup_course(connection.device.as_ref(), &connection.manifest, &transfer).await?;
    assert_eq!(fs::read(&unrelated)?, unrelated_bytes);
    assert!(!renamed.exists());
    deliver_workout(
        connection.device.as_ref(),
        &connection.manifest,
        root.path(),
    )
    .await?;
    Ok(())
}

fn rewritten_course(payload: &[u8], name: &str) -> Result<Vec<u8>> {
    use garmin_model::route::{
        Elevation, RevisionProvenance, RevisionSource, RoutePlanId, RoutePlanRevision,
        RoutePlanRevisionId, RoutePoint, RouteShape,
    };
    let course = garmin_fit::course::decode(payload)?;
    let points = course
        .points()
        .iter()
        .map(|point| {
            Ok(RoutePoint::from_parts(
                point.coordinate(),
                Some(Elevation::from_meters(273.0)?),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let revision = RoutePlanRevision::from_parts(
        RoutePlanRevisionId::new_v4(),
        RoutePlanId::new_v4(),
        None,
        Timestamp::from_unix_seconds(1_780_000_000)?,
        name.parse()?,
        course.sport(),
        RouteShape::from_geometry(points)?,
        course.cues().to_vec(),
        RevisionProvenance::from_parts(RevisionSource::Freehand, Vec::new()),
    )?;
    Ok(garmin_fit::course::encode(
        &revision,
        garmin_fit::course::SerialNumber::from_u32(1)?,
    )?)
}

#[tokio::test]
async fn cleanup_refuses_duplicate_exact_copies() -> Result<()> {
    let root = tempfile::tempdir()?;
    let watch = Device::open(root.path().join("watch"))?;
    let connector: Arc<dyn Connector> = Arc::new(DirectoryConnector {
        root: watch.root().to_owned(),
        storage_id: fixture::STORAGE_ID.to_owned(),
        storage_label: fixture::STORAGE_LABEL.to_owned(),
    });
    let host = root.path().join("host");
    recover_course(&host, fixture::KEY, Arc::clone(&connector)).await?;
    let transfer = fs::read_dir(host.join("course-transfers"))?
        .next()
        .context("missing transfer")??
        .path();
    let intent = read_intent(&transfer)?;
    let duplicate = watch.root().join("Garmin/Courses/duplicate.fit");
    fs::copy(transfer.join("payload.fit"), &duplicate)?;
    let connection = connector.connect().await?;
    assert!(
        cleanup_course(connection.device.as_ref(), &connection.manifest, &transfer)
            .await
            .is_err()
    );
    assert!(duplicate.exists());
    assert!(watch.root().join(intent.path.as_path()).exists());
    Ok(())
}

#[tokio::test]
#[ignore = "physical workout upload and verified Course deletion require owner consent and a selected model"]
async fn physical_workout_delivery_and_course_cleanup() -> Result<()> {
    let model = std::env::var("GARMIN_TOOLKIT_ACCEPTANCE_MODEL")?;
    let output = PathBuf::from(std::env::var("GARMIN_TOOLKIT_WORKOUT_ACCEPTANCE_DIR")?);
    let transfer = std::env::var_os("GARMIN_TOOLKIT_COURSE_TRANSFER_DIR").map(PathBuf::from);
    fs::create_dir(&output)?;
    let candidates = garmin_device::discover_mounted_mtp()
        .into_iter()
        .filter(|candidate| candidate.name == model)
        .collect::<Vec<_>>();
    let [candidate] = candidates.as_slice() else {
        bail!("expected exactly one selected mounted device");
    };
    let connection = MountedConnector(candidate.mount_id.clone())
        .connect()
        .await?;
    ensure!(
        connection.manifest.summary.model == model,
        "selected model changed"
    );
    if let Some(transfer) = transfer {
        cleanup_course(connection.device.as_ref(), &connection.manifest, &transfer).await?;
        fs::write(
            output.join("course-cleanup-complete"),
            b"TK Recovery deleted by verified digest\n",
        )?;
    }
    deliver_workout(connection.device.as_ref(), &connection.manifest, &output).await?;
    Ok(())
}

async fn cleanup_course<D: DeviceWrite + ?Sized>(
    device: &D,
    manifest: &DeviceManifest,
    transfer: &Path,
) -> Result<()> {
    let intent = read_intent(transfer)?;
    ensure!(
        manifest.identity_digest() == intent.review.target.device_digest,
        "Course receipt belongs to another device"
    );
    let payload = fs::read(transfer.join("payload.fit"))?;
    ensure!(
        payload.len() as u64 == intent.review.byte_count.as_u64()
            && hex::encode(Sha256::digest(&payload)) == intent.sha256,
        "retained Course payload changed"
    );
    let decoded = garmin_fit::course::decode(&payload)?;
    ensure!(
        decoded.name().as_str() == "TK Recovery",
        "receipt is not the synthetic cleanup target"
    );
    let storage = &intent.review.target.storage_id;
    let mut paths = vec![intent.path];
    for capability in manifest.capabilities().capabilities() {
        let Some(source) = capability.course_source(&intent.review.file_name) else {
            continue;
        };
        let parent = source
            .as_path()
            .parent()
            .context("missing Course directory")?;
        let directory = SafeRelativePath::parse(parent)?;
        if device.inspect(storage, &directory).await? != DevicePathStatus::Directory {
            continue;
        }
        let entries = device.list_directory(storage, &directory).await?;
        ensure!(
            entries.len() <= 4096,
            "Course directory exceeds acceptance bound"
        );
        for entry in entries {
            if entry.state != DevicePathState::RegularFile
                || entry.size.is_none_or(|size| size > 1024 * 1024)
            {
                continue;
            }
            let path = SafeRelativePath::parse(parent.join(&entry.name))?;
            if capability.accepts(path.as_path()) && !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    let mut matches = Vec::new();
    for path in paths {
        let DevicePathStatus::RegularFile { size } = device.inspect(storage, &path).await? else {
            continue;
        };
        if size > 1024 * 1024 {
            continue;
        }
        let Some(bytes) = device.read_bounded_file(storage, &path, size).await? else {
            continue;
        };
        // Firmware supplies elevations missing from this synthetic Course.
        // Require its name, sport, every coordinate and every cue to match.
        if bytes == payload
            || garmin_fit::course::decode(&bytes).is_ok_and(|course| {
                course.name() == decoded.name()
                    && course.sport() == decoded.sport()
                    && course.cues() == decoded.cues()
                    && course
                        .points()
                        .iter()
                        .map(garmin_model::route::RoutePoint::coordinate)
                        .eq(decoded
                            .points()
                            .iter()
                            .map(garmin_model::route::RoutePoint::coordinate))
            })
        {
            matches.push((path, size, hex::encode(Sha256::digest(&bytes))));
        }
    }
    let [(path, size, sha256)] = matches.as_slice() else {
        bail!("TK Recovery has no unique matching Course; no file deleted");
    };
    device.delete(storage, path, *size, sha256).await?;
    ensure!(
        device.inspect(storage, path).await? == DevicePathStatus::Missing,
        "Course deletion was not confirmed"
    );
    println!("Removed the verified TK Recovery Course; unrelated courses were not selected.");
    Ok(())
}

async fn deliver_workout<D: DeviceWrite + ?Sized>(
    device: &D,
    manifest: &DeviceManifest,
    output: &Path,
) -> Result<()> {
    let serial = u32::from_be_bytes(Uuid::new_v4().as_bytes()[..4].try_into()?);
    let (name, bytes) = workout::cycling(Header::Checksummed, serial);
    let file_name = format!("tkw-{}.fit", Uuid::new_v4());
    let destinations = manifest
        .capabilities()
        .capabilities()
        .iter()
        .filter_map(|capability| capability.workout_destination(&file_name))
        .collect::<Vec<_>>();
    let [path] = destinations.as_slice() else {
        bail!("requires exactly one declared workout input");
    };
    let storage = device.primary_storage_id().await?;
    ensure!(
        device.inspect(&storage, path).await? == DevicePathStatus::Missing,
        "workout destination is occupied"
    );
    let pairing = garmin_update::read_profile_marker(device, &storage)
        .await?
        .map(|marker| marker.bytes())
        .transpose()?;
    let source = output.join("workout.fit");
    fs::write(&source, &bytes)?;
    fs::write(output.join("workout-name.txt"), &name)?;
    let sha256 = hex::encode(Sha256::digest(&bytes));
    device
        .upload(
            &storage,
            path,
            &source,
            bytes.len() as u64,
            &sha256,
            MountedMtpUploadProgress {
                reporter: garmin_progress::ProgressReporter::default(),
                completed_before: 0,
                total: bytes.len() as u64,
            },
        )
        .await?;
    device
        .verify(&storage, path, bytes.len() as u64, &sha256)
        .await?;
    ensure!(
        device
            .read_bounded_file(&storage, path, bytes.len() as u64)
            .await?
            == Some(bytes),
        "workout readback differs"
    );
    let after = garmin_update::read_profile_marker(device, &storage)
        .await?
        .map(|marker| marker.bytes())
        .transpose()?;
    ensure!(after == pairing, "pairing changed during workout delivery");
    println!(
        "Workout {name}: cycling, 60 seconds active, 30 seconds rest, open targets; exact readback passed."
    );
    Ok(())
}
