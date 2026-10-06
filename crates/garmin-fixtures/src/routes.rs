//! Saved demo routes exported from the existing, attributed recordings.

use garmin_fit::fixture::ActivityCase;
use garmin_importer::{RouteImportReceipt, RouteImportRequest, RouteImporter};
use garmin_model::{
    activity::TrackPoint,
    artifact::AcquisitionOperationId,
    identity::{Source, SourceId},
    route::{RouteCandidateSource, RouteSport},
    value::Timestamp,
};
use garmin_storage::Storage;
use gpx_format::{Gpx, GpxVersion, Track, TrackSegment, Waypoint};

use super::{SeedError, SeededCorpus, invalid, parse};

#[cfg(test)]
mod tests;

const CASES: [(ActivityCase, &str, RouteSport); 3] = [
    (ActivityCase::CityRide, "City ride", RouteSport::Cycling),
    (
        ActivityCase::NeighborhoodWalk,
        "Neighborhood walk",
        RouteSport::Walking,
    ),
    (
        ActivityCase::MountainHike,
        "Mountain hike",
        RouteSport::Hiking,
    ),
];

pub(super) async fn seed(
    storage: &Storage,
    corpus: &SeededCorpus,
) -> Result<Vec<RouteImportReceipt>, SeedError> {
    let owner = corpus
        .users()
        .first()
        .ok_or_else(|| invalid("route owner", "Alex is missing"))?
        .id();
    let source = Source::from_parts(
        SourceId::from_u128(0x3100_0000_0000_4000_8000_0000_0000_0001),
        owner,
        parse("route source", "Demo recording exports")?,
        None,
    );
    let mut receipts = Vec::with_capacity(CASES.len());
    for (index, (case, name, sport)) in CASES.into_iter().enumerate() {
        let operation = AcquisitionOperationId::from_u128(
            0x4100_0000_0000_4000_8000_0000_0000_0001 + index as u128,
        );
        // Do not regenerate artifacts or replace an edited route head on restart.
        if let Some(import) = storage.route_import(owner, operation).await? {
            receipts.push(import.receipt());
            continue;
        }
        let activity = corpus
            .activities()
            .iter()
            .find(|activity| activity.case() == case)
            .ok_or_else(|| invalid("route recording", case.file_name()))?;
        let [observation] = activity.receipt().observation_ids() else {
            return Err(invalid("route recording", "expected one activity"));
        };
        let stored = storage
            .activity(activity.owner_id(), *observation)
            .await?
            .ok_or_else(|| invalid("route recording", "activity is missing"))?;
        let bytes = export(stored.normalized().activity().track(), case, name)?;
        let request = RouteImportRequest::from_parts(
            owner,
            &source,
            parse(
                "route filename",
                &format!("{}.gpx", case.file_name().trim_end_matches(".fit")),
            )?,
            operation,
            Timestamp::from_unix_seconds(case.end())
                .map_err(|error| invalid("route acquisition time", error))?,
            &bytes,
            RouteCandidateSource::TrackSegment {
                track: 0,
                segment: 0,
            },
            parse("route name", name)?,
            sport,
        );
        receipts.push(RouteImporter::new(storage).import(request).await?);
    }
    Ok(receipts)
}

fn export(points: &[TrackPoint], case: ActivityCase, name: &str) -> Result<Vec<u8>, SeedError> {
    let points = points
        .iter()
        .map(|point| {
            // These three recordings are continuous: never silently bridge a missing fix.
            let coordinate = point.coordinate().ok_or_else(|| {
                invalid("route geometry", "recording contains a missing coordinate")
            })?;
            let mut waypoint = Waypoint::new(coordinate.as_geo().into());
            waypoint.elevation = point.elevation().map(|height| height.as_meters());
            Ok(waypoint)
        })
        .collect::<Result<Vec<_>, SeedError>>()?;
    let document = Gpx {
        version: GpxVersion::Gpx11,
        creator: Some("Garmin Toolkit demo recording export".into()),
        tracks: vec![Track {
            name: Some(name.into()),
            source: Some(format!("Development recording: {}", case.file_name())),
            segments: vec![TrackSegment { points }],
            ..Track::default()
        }],
        ..Gpx::default()
    };
    let mut bytes = Vec::new();
    gpx_format::write(&document, &mut bytes).map_err(|error| invalid("route GPX", error))?;
    Ok(bytes)
}
