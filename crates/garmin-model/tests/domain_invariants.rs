//! Cross-module domain invariants.

use std::error::Error;

use garmin_color::Color;
use garmin_model::{
    artifact::{
        Acquisition, AcquisitionId, AcquisitionOperationId, Artifact, ArtifactDigest, ArtifactId,
        ByteCount, MediaType, NormalizationFailure, NormalizationOutcome, NormalizationRun,
        NormalizationRunId, SchemaVersion, SourceIdentity,
    },
    identity::{
        AvatarArtifactId, Device, DeviceId, DeviceLabel, DisplayName, ImageDimensions, Profile,
        Role, Source, SourceId, SourceLabel, User, UserId,
    },
    observation::{
        AssociationBasis, AssociationGroup, AssociationGroupId, FingerprintSchema, Observation,
        ObservationId, ObservationKind, SemanticFingerprint,
    },
    route::{
        Coordinate, CueText, Elevation, Latitude, Longitude, NavigationCue, RevisionProvenance,
        RevisionSource, RouteName, RoutePlan, RoutePlanId, RoutePlanRevision, RoutePlanRevisionId,
        RoutePoint, RoutePointIndex, RouteShape, RouteSport,
    },
    value::{ComponentVersion, Timestamp, Transformation},
};
use semver::Version;

type TestResult = Result<(), Box<dyn Error>>;

fn source(owner_id: UserId) -> Result<Source, garmin_model::identity::Error> {
    Ok(Source::from_parts(
        SourceId::new_v4(),
        owner_id,
        "USB watch".parse::<SourceLabel>()?,
        None,
    ))
}

fn normalization_run(
    owner_id: UserId,
    outcome: NormalizationOutcome,
) -> Result<NormalizationRun, Box<dyn Error>> {
    let source = source(owner_id)?;
    let acquisition = Acquisition::from_source(
        AcquisitionId::new_v4(),
        AcquisitionOperationId::new_v4(),
        ArtifactId::new_v4(),
        &source,
        "GARMIN/ACTIVITY/1.FIT".parse()?,
        Timestamp::from_jiff(jiff::Timestamp::UNIX_EPOCH),
    );
    Ok(NormalizationRun::from_parts(
        NormalizationRunId::new_v4(),
        &acquisition,
        ComponentVersion::from_parts("fit-normalizer", Version::new(1, 0, 0))?,
        SchemaVersion::from_u32(1)?,
        Vec::new(),
        outcome,
    ))
}

fn fingerprint(bytes: &[u8]) -> Result<SemanticFingerprint, garmin_model::value::Error> {
    Ok(SemanticFingerprint::from_canonical_bytes(
        FingerprintSchema::from_component(ComponentVersion::from_parts(
            "activity-fingerprint",
            Version::new(1, 0, 0),
        )?),
        bytes,
    ))
}

fn observation(
    owner_id: UserId,
    kind: ObservationKind,
    fingerprint: SemanticFingerprint,
) -> Result<Observation, Box<dyn Error>> {
    let run = normalization_run(owner_id, NormalizationOutcome::Succeeded)?;
    Ok(Observation::from_run(
        ObservationId::new_v4(),
        &run,
        kind,
        None,
        fingerprint,
    )?)
}

fn route_point(latitude: f64, longitude: f64) -> Result<RoutePoint, garmin_model::route::Error> {
    Ok(RoutePoint::from_parts(
        Coordinate::from_parts(
            Latitude::from_degrees(latitude)?,
            Longitude::from_degrees(longitude)?,
        ),
        None,
    ))
}

#[test]
fn artifact_deduplication_uses_exact_bytes_not_record_ids() -> TestResult {
    let bytes = b"same immutable bytes";
    let media_type = "APPLICATION/FIT".parse::<MediaType>()?;
    let first = Artifact::from_bytes(ArtifactId::new_v4(), media_type.clone(), bytes);
    let second = Artifact::from_bytes(ArtifactId::new_v4(), media_type, bytes);

    assert_ne!(first.id(), second.id());
    assert_eq!(first.digest(), second.digest());
    assert_eq!(first.media_type().to_string(), "application/fit");
    assert_eq!(first.byte_count().as_u64(), bytes.len().try_into()?);
    Ok(())
}

#[test]
fn opaque_source_identity_is_preserved() -> TestResult {
    let identity = SourceIdentity::from_string("  device value  ".to_owned())?;

    assert_eq!(identity.as_str(), "  device value  ");
    assert!(SourceIdentity::from_string("\t".to_owned()).is_err());
    Ok(())
}

#[test]
fn schema_versions_start_at_one() -> TestResult {
    assert_eq!(
        SchemaVersion::from_u32(0),
        Err(garmin_model::artifact::Error::ZeroSchemaVersion)
    );
    let version = SchemaVersion::from_u32(1)?;
    assert_eq!(version.as_u32(), 1);
    assert_eq!(version.into_u32(), 1);
    Ok(())
}

#[test]
fn failed_normalization_preserves_provenance_without_observation() -> TestResult {
    let failure = NormalizationFailure::from_string("invalid checksum".to_owned())?;
    let run = normalization_run(UserId::new_v4(), NormalizationOutcome::Failed(failure))?;

    let result = Observation::from_run(
        ObservationId::new_v4(),
        &run,
        ObservationKind::Activity,
        None,
        fingerprint(b"known fields")?,
    );

    assert_eq!(
        result,
        Err(garmin_model::observation::Error::FailedNormalization)
    );
    assert!(!run.succeeded());
    Ok(())
}

#[test]
fn associations_never_cross_users_or_merge_members() -> TestResult {
    let semantic_fingerprint = fingerprint(b"same normalized activity")?;
    let first = observation(
        UserId::new_v4(),
        ObservationKind::Activity,
        semantic_fingerprint.clone(),
    )?;
    let other_user = observation(
        UserId::new_v4(),
        ObservationKind::Activity,
        semantic_fingerprint.clone(),
    )?;

    let cross_user = AssociationGroup::from_observations(
        AssociationGroupId::new_v4(),
        AssociationBasis::EqualFingerprint(semantic_fingerprint),
        &[first, other_user],
    );

    assert_eq!(
        cross_user,
        Err(garmin_model::observation::Error::MixedUsers)
    );
    Ok(())
}

#[test]
fn exact_association_requires_distinct_equal_observations() -> TestResult {
    let owner_id = UserId::new_v4();
    let shared = fingerprint(b"same normalized activity")?;
    let first = observation(owner_id, ObservationKind::Activity, shared.clone())?;
    let second = observation(owner_id, ObservationKind::Activity, shared.clone())?;
    let unequal = observation(
        owner_id,
        ObservationKind::Activity,
        fingerprint(b"different normalized activity")?,
    )?;
    let other_kind = observation(owner_id, ObservationKind::Measurement, shared.clone())?;

    let group = AssociationGroup::from_observations(
        AssociationGroupId::new_v4(),
        AssociationBasis::EqualFingerprint(shared.clone()),
        &[first.clone(), second],
    )?;
    let duplicate = AssociationGroup::from_observations(
        AssociationGroupId::new_v4(),
        AssociationBasis::EqualFingerprint(shared.clone()),
        &[first.clone(), first.clone()],
    );
    let mixed_kinds = AssociationGroup::from_observations(
        AssociationGroupId::new_v4(),
        AssociationBasis::EqualFingerprint(shared.clone()),
        &[first.clone(), other_kind],
    );
    let mismatch = AssociationGroup::from_observations(
        AssociationGroupId::new_v4(),
        AssociationBasis::EqualFingerprint(shared),
        &[first, unequal],
    );

    assert_eq!(group.members().len(), 2);
    assert_eq!(
        duplicate,
        Err(garmin_model::observation::Error::TooFewAssociationMembers)
    );
    assert_eq!(
        mismatch,
        Err(garmin_model::observation::Error::FingerprintMismatch)
    );
    assert_eq!(
        mixed_kinds,
        Err(garmin_model::observation::Error::MixedKinds)
    );
    Ok(())
}

#[test]
fn coordinate_conversions_preserve_axis_order() -> Result<(), garmin_model::route::Error> {
    let coordinate = Coordinate::from_geo(geo_types::coord! { x: 24.9384, y: 60.1699 })?;

    assert_eq!(
        coordinate.latitude().as_degrees().to_bits(),
        60.1699_f64.to_bits()
    );
    assert_eq!(
        coordinate.longitude().as_degrees().to_bits(),
        24.9384_f64.to_bits()
    );
    assert_eq!(
        coordinate.into_geo(),
        geo_types::coord! { x: 24.9384, y: 60.1699 }
    );
    assert!(Latitude::from_degrees(f64::NAN).is_err());
    assert!(Longitude::from_degrees(181.0).is_err());
    assert!(Elevation::from_meters(f64::INFINITY).is_err());
    Ok(())
}

#[test]
fn route_revisions_validate_shapes_cues_and_plan_identity() -> TestResult {
    let plan_id = RoutePlanId::new_v4();
    let first_revision_id = RoutePlanRevisionId::new_v4();
    let shape =
        RouteShape::from_geometry(vec![route_point(60.0, 24.0)?, route_point(60.1, 24.1)?])?;
    let valid_cue =
        NavigationCue::from_parts(RoutePointIndex::from_usize(1), "Turn".parse::<CueText>()?);
    let invalid_cue = NavigationCue::from_parts(
        RoutePointIndex::from_usize(2),
        "Too late".parse::<CueText>()?,
    );

    let invalid_revision = RoutePlanRevision::from_parts(
        first_revision_id,
        plan_id,
        None,
        Timestamp::from_jiff(jiff::Timestamp::UNIX_EPOCH),
        "Morning ride".parse::<RouteName>()?,
        RouteSport::Cycling,
        shape.clone(),
        vec![invalid_cue],
        RevisionProvenance::from_parts(RevisionSource::Freehand, Vec::new()),
    );
    assert_eq!(
        invalid_revision,
        Err(garmin_model::route::Error::CueOutsideShape)
    );

    let self_referential = RoutePlanRevision::from_parts(
        first_revision_id,
        plan_id,
        Some(first_revision_id),
        Timestamp::from_jiff(jiff::Timestamp::UNIX_EPOCH),
        "Loop".parse::<RouteName>()?,
        RouteSport::Cycling,
        shape.clone(),
        Vec::new(),
        RevisionProvenance::from_parts(RevisionSource::Freehand, Vec::new()),
    );
    assert_eq!(
        self_referential,
        Err(garmin_model::route::Error::SelfReferentialRevision)
    );

    let revision = RoutePlanRevision::from_parts(
        RoutePlanRevisionId::new_v4(),
        plan_id,
        Some(first_revision_id),
        Timestamp::from_jiff(jiff::Timestamp::UNIX_EPOCH),
        "Morning ride".parse::<RouteName>()?,
        RouteSport::Cycling,
        shape,
        vec![valid_cue],
        RevisionProvenance::from_parts(RevisionSource::Freehand, Vec::new()),
    )?;
    let plan = RoutePlan::from_parts(plan_id, UserId::new_v4(), first_revision_id);
    assert_eq!(
        plan.with_revision(&revision)?.current_revision_id(),
        revision.id()
    );

    let foreign_revision = RoutePlanRevision::from_parts(
        RoutePlanRevisionId::new_v4(),
        RoutePlanId::new_v4(),
        None,
        Timestamp::from_jiff(jiff::Timestamp::UNIX_EPOCH),
        "Foreign".parse::<RouteName>()?,
        RouteSport::Running,
        RouteShape::from_control_points(vec![route_point(60.0, 24.0)?])?,
        Vec::new(),
        RevisionProvenance::from_parts(RevisionSource::Freehand, Vec::new()),
    )?;
    assert_eq!(
        plan.with_revision(&foreign_revision),
        Err(garmin_model::route::Error::RevisionForAnotherPlan)
    );

    let stale_revision = RoutePlanRevision::from_parts(
        RoutePlanRevisionId::new_v4(),
        plan_id,
        None,
        Timestamp::from_jiff(jiff::Timestamp::UNIX_EPOCH),
        "Stale".parse::<RouteName>()?,
        RouteSport::Running,
        RouteShape::from_control_points(vec![route_point(60.0, 24.0)?])?,
        Vec::new(),
        RevisionProvenance::from_parts(RevisionSource::Freehand, Vec::new()),
    )?;
    assert_eq!(
        plan.with_revision(&stale_revision),
        Err(garmin_model::route::Error::RevisionDoesNotFollowHead)
    );
    Ok(())
}

#[test]
fn artifact_values_and_provenance_round_trip() -> TestResult {
    let parsed_media_type = "application/fit".parse::<mediatype::MediaTypeBuf>()?;
    let media_type = MediaType::from_media_type(parsed_media_type.clone());
    assert_eq!(media_type.as_media_type(), &parsed_media_type);
    assert_eq!(media_type.into_media_type(), parsed_media_type);

    let digest = ArtifactDigest::from_bytes(b"activity");
    let parsed_digest = digest.to_string().parse::<ArtifactDigest>()?;
    assert_eq!(parsed_digest.as_blake3(), digest.as_blake3());
    assert_eq!(
        ArtifactDigest::from_blake3(digest.into_blake3()),
        parsed_digest
    );

    let byte_count = ByteCount::from_u64(8);
    assert_eq!(byte_count.to_string(), "8 B");
    assert_eq!(byte_count.into_u64(), 8);

    let owner_id = UserId::new_v4();
    let source = source(owner_id)?;
    let operation_id = AcquisitionOperationId::new_v4();
    let source_id = source.id();
    let acquired_at = Timestamp::from_jiff(jiff::Timestamp::UNIX_EPOCH);
    let source_identity = "GARMIN/ACTIVITY/1.FIT".parse::<SourceIdentity>()?;
    assert_eq!(source_identity.to_string(), "GARMIN/ACTIVITY/1.FIT");
    assert_eq!(
        source_identity.clone().into_string(),
        "GARMIN/ACTIVITY/1.FIT"
    );

    let acquisition = Acquisition::from_source(
        AcquisitionId::new_v4(),
        operation_id,
        ArtifactId::new_v4(),
        &source,
        source_identity,
        acquired_at,
    );
    assert_eq!(acquisition.operation_id(), operation_id);
    assert_eq!(acquisition.source_id(), source_id);
    assert_eq!(
        acquisition.source_identity().as_str(),
        "GARMIN/ACTIVITY/1.FIT"
    );
    assert_eq!(acquisition.acquired_at(), acquired_at);

    let parser = ComponentVersion::from_parts("fit-normalizer", Version::new(1, 2, 3))?;
    let transformation = Transformation::from_component(ComponentVersion::from_parts(
        "semicircle-to-degrees",
        Version::new(2, 0, 0),
    )?);
    let run = NormalizationRun::from_parts(
        NormalizationRunId::new_v4(),
        &acquisition,
        parser.clone(),
        SchemaVersion::from_u32(2)?,
        vec![transformation.clone()],
        NormalizationOutcome::Succeeded,
    );
    assert_eq!(run.acquisition_id(), acquisition.id());
    assert_eq!(run.artifact_id(), acquisition.artifact_id());
    assert_eq!(run.parser(), &parser);
    assert_eq!(run.schema().to_string(), "2");
    assert_eq!(run.transformations(), &[transformation]);
    assert_eq!(run.outcome(), &NormalizationOutcome::Succeeded);

    let failure = " invalid checksum ".parse::<NormalizationFailure>()?;
    assert_eq!(failure.as_str(), "invalid checksum");
    assert_eq!(failure.to_string(), "invalid checksum");
    assert_eq!(failure.into_string(), "invalid checksum");
    assert!(NormalizationFailure::from_string(" ".to_owned()).is_err());
    Ok(())
}

#[test]
fn identity_and_shared_values_round_trip() -> TestResult {
    let display_name = DisplayName::from_string(" Rider ".to_owned())?;
    let accent = Color::from_rgb(0x45, 0x89, 0xff);
    let avatar = AvatarArtifactId::from_artifact_id(ArtifactId::new_v4());
    let profile = Profile::from_parts(display_name.clone(), Some(accent), Some(avatar));
    assert_eq!(display_name.to_string(), "Rider");
    assert_eq!(display_name.clone().into_string(), "Rider");
    assert_eq!(profile.display_name(), &display_name);
    assert_eq!(profile.accent(), Some(accent));
    assert_eq!(profile.avatar_artifact_id(), Some(avatar));
    assert_eq!(avatar.as_artifact_id(), avatar.into_artifact_id());
    let dimensions = ImageDimensions::from_width_height(640, 480)?;
    assert_eq!(dimensions.into_width_height(), (640, 480));
    assert!(ImageDimensions::from_width_height(0, 480).is_err());
    assert!(ImageDimensions::from_width_height(640, 0).is_err());

    let owner_id = UserId::new_v4();
    let user = User::from_parts(
        owner_id,
        Role::Member,
        Profile::from_display_name("Rider".parse()?),
    );
    assert_eq!(user.id(), owner_id);

    let device_id = DeviceId::new_v4();
    let device_label = DeviceLabel::from_string(" Edge 1050 ".to_owned())?;
    assert_eq!(device_label.to_string(), "Edge 1050");
    assert_eq!(device_label.clone().into_string(), "Edge 1050");
    let device = Device::from_parts(device_id, device_label);
    assert_eq!(device.id(), device_id);
    assert_eq!(device.label().as_str(), "Edge 1050");

    let source_label = SourceLabel::from_string(" USB ".to_owned())?;
    assert_eq!(source_label.to_string(), "USB");
    assert_eq!(source_label.clone().into_string(), "USB");
    let source = Source::from_parts(SourceId::new_v4(), owner_id, source_label, Some(device_id));
    assert_eq!(source.label().as_str(), "USB");
    assert_eq!(source.device_id(), Some(device_id));

    let timestamp = "2026-08-31T12:00:00Z".parse::<Timestamp>()?;
    assert_eq!(timestamp.to_string(), "2026-08-31T12:00:00Z");
    assert_eq!(timestamp.as_jiff(), &timestamp.into_jiff());

    let component = ComponentVersion::from_parts(" parser ", Version::new(1, 2, 3))?;
    assert_eq!(component.name(), "parser");
    assert_eq!(component.version(), &Version::new(1, 2, 3));
    assert_eq!(component.to_string(), "parser@1.2.3");
    let (name, version) = component.into_parts();
    assert_eq!(name, "parser");
    assert_eq!(version, Version::new(1, 2, 3));
    assert!(ComponentVersion::from_parts(" ", Version::new(1, 0, 0)).is_err());

    let transformation = Transformation::from_component(ComponentVersion::from_parts(
        "projection",
        Version::new(4, 0, 0),
    )?);
    assert_eq!(transformation.as_component().name(), "projection");
    assert_eq!(transformation.to_string(), "projection@4.0.0");
    assert_eq!(transformation.into_component().name(), "projection");
    Ok(())
}

#[test]
fn observation_contract_exposes_normalized_identity() -> TestResult {
    let owner_id = UserId::new_v4();
    let schema_component = ComponentVersion::from_parts("activity", Version::new(1, 0, 0))?;
    let schema = FingerprintSchema::from_component(schema_component.clone());
    assert_eq!(schema.as_component(), &schema_component);
    assert_eq!(schema.to_string(), "activity@1.0.0");
    assert_eq!(schema.clone().into_component(), schema_component);

    let digest = blake3::hash(b"canonical activity");
    let semantic_fingerprint = SemanticFingerprint::from_parts(schema, digest);
    assert_eq!(semantic_fingerprint.schema().to_string(), "activity@1.0.0");
    assert_eq!(semantic_fingerprint.digest(), &digest);
    assert_eq!(
        semantic_fingerprint.to_string(),
        format!("activity@1.0.0:{digest}")
    );
    let (schema, round_trip_digest) = semantic_fingerprint.clone().into_parts();
    assert_eq!(schema.to_string(), "activity@1.0.0");
    assert_eq!(round_trip_digest, digest);

    let observed_at = Timestamp::from_jiff(jiff::Timestamp::UNIX_EPOCH);
    let run = normalization_run(owner_id, NormalizationOutcome::Succeeded)?;
    let run_id = run.id();
    let device_observation = Observation::from_run(
        ObservationId::new_v4(),
        &run,
        ObservationKind::Device,
        Some(observed_at),
        semantic_fingerprint,
    )?;
    assert_eq!(device_observation.owner_id(), owner_id);
    assert_eq!(device_observation.kind(), ObservationKind::Device);
    assert_eq!(device_observation.observed_at(), Some(observed_at));
    assert_eq!(device_observation.normalization_run_id(), run_id);

    assert_eq!(
        AssociationGroup::from_observations(
            AssociationGroupId::new_v4(),
            AssociationBasis::UserConfirmed,
            &[],
        ),
        Err(garmin_model::observation::Error::TooFewAssociationMembers)
    );

    let shared = fingerprint(b"similar observations")?;
    let first = observation(owner_id, ObservationKind::Measurement, shared.clone())?;
    let second = observation(owner_id, ObservationKind::Measurement, shared)?;
    let group_id = AssociationGroupId::new_v4();
    let group = AssociationGroup::from_observations(
        group_id,
        AssociationBasis::UserConfirmed,
        &[first, second],
    )?;
    assert_eq!(group.id(), group_id);
    assert_eq!(group.owner_id(), owner_id);
    assert_eq!(group.kind(), ObservationKind::Measurement);
    assert_eq!(group.basis(), &AssociationBasis::UserConfirmed);
    Ok(())
}

#[test]
fn route_values_and_revisions_expose_their_complete_boundary() -> TestResult {
    let latitude = Latitude::from_degrees(60.1699)?;
    assert_eq!(latitude.to_string(), "60.1699000°");
    assert_eq!(latitude.into_degrees().to_bits(), 60.1699_f64.to_bits());
    let longitude = Longitude::from_degrees(24.9384)?;
    assert_eq!(longitude.to_string(), "24.9384000°");
    assert_eq!(longitude.into_degrees().to_bits(), 24.9384_f64.to_bits());
    let elevation = Elevation::from_meters(42.5)?;
    assert_eq!(elevation.as_meters().to_bits(), 42.5_f64.to_bits());
    assert_eq!(elevation.to_string(), "42.5 m");
    assert_eq!(elevation.into_meters().to_bits(), 42.5_f64.to_bits());

    let coordinate = Coordinate::from_parts(latitude, longitude);
    assert_eq!(
        coordinate.as_geo(),
        geo_types::coord! { x: 24.9384, y: 60.1699 }
    );
    assert_eq!(coordinate.to_string(), "60.1699000°, 24.9384000°");
    let point = RoutePoint::from_parts(coordinate, Some(elevation));
    assert_eq!(point.coordinate(), coordinate);
    assert_eq!(point.elevation(), Some(elevation));
    assert_eq!(point.into_parts(), (coordinate, Some(elevation)));

    assert_eq!(
        RouteShape::from_geometry(vec![point]),
        Err(garmin_model::route::Error::TooFewGeometryPoints)
    );
    assert_eq!(
        RouteShape::from_control_points(Vec::new()),
        Err(garmin_model::route::Error::NoControlPoints)
    );
    let shape = RouteShape::from_geometry(vec![point, point])?;
    assert!(shape.is_geometry());

    let point_index = RoutePointIndex::from_usize(1);
    assert_eq!(point_index.to_string(), "1");
    assert_eq!(point_index.into_usize(), 1);
    let cue_text = CueText::from_string(" Turn left ".to_owned())?;
    assert_eq!(cue_text.to_string(), "Turn left");
    assert_eq!(cue_text.clone().into_string(), "Turn left");
    let cue = NavigationCue::from_parts(point_index, cue_text);
    assert_eq!(cue.point_index(), point_index);
    assert_eq!(cue.text().as_str(), "Turn left");

    let transformation = Transformation::from_component(ComponentVersion::from_parts(
        "snap",
        Version::new(1, 0, 0),
    )?);
    let provenance = RevisionProvenance::from_parts(
        RevisionSource::Artifact(ArtifactId::new_v4()),
        vec![transformation.clone()],
    );
    assert!(matches!(provenance.source(), RevisionSource::Artifact(_)));
    assert_eq!(provenance.transformations(), &[transformation]);
    let (source, transformations) = provenance.clone().into_parts();
    assert!(matches!(source, RevisionSource::Artifact(_)));
    assert_eq!(transformations.len(), 1);

    let plan_id = RoutePlanId::new_v4();
    let previous_id = RoutePlanRevisionId::new_v4();
    let created_at = Timestamp::from_jiff(jiff::Timestamp::UNIX_EPOCH);
    let route_name = RouteName::from_string(" Morning ride ".to_owned())?;
    assert_eq!(route_name.to_string(), "Morning ride");
    assert_eq!(route_name.clone().into_string(), "Morning ride");
    let revision = RoutePlanRevision::from_parts(
        RoutePlanRevisionId::new_v4(),
        plan_id,
        Some(previous_id),
        created_at,
        route_name,
        RouteSport::Cycling,
        shape,
        vec![cue],
        provenance,
    )?;
    assert_eq!(revision.plan_id(), plan_id);
    assert_eq!(revision.previous_id(), Some(previous_id));
    assert_eq!(revision.created_at(), created_at);
    assert_eq!(revision.name().as_str(), "Morning ride");
    assert_eq!(revision.sport(), RouteSport::Cycling);
    assert!(revision.shape().is_geometry());
    assert_eq!(revision.cues().len(), 1);
    assert!(matches!(
        revision.provenance().source(),
        RevisionSource::Artifact(_)
    ));

    let owner_id = UserId::new_v4();
    let plan = RoutePlan::from_parts(plan_id, owner_id, previous_id);
    assert_eq!(plan.id(), plan_id);
    assert_eq!(plan.owner_id(), owner_id);
    Ok(())
}
