//! Target-neutral application services.

pub mod maps;

use garmin_fit::{CreatorDiagnostics, NormalizedActivity};
use garmin_importer::{
    AvatarCrop, AvatarImportReceipt, AvatarImportRequest as ImporterAvatarRequest, AvatarImporter,
    FitImportOutcome, FitImportRequest as ImporterRequest, FitImporter, RouteImportReceipt,
    RouteImportRequest as ImporterRouteRequest, RouteImporter,
};
use garmin_model::{
    activity::ActivitySummary,
    artifact::{AcquisitionOperationId, ArtifactId, NormalizationFailure, SourceIdentity},
    identity::{DisplayName, Profile, ProfilePreferences, Role, Source, User, UserId},
    observation::ObservationId,
    route::{RouteName, RoutePlanId, RouteSport},
    value::Timestamp,
};
use garmin_storage::{
    Storage, StoredActivity, StoredActivitySummary, StoredRoutePlan, StoredRoutePlanSummary,
};
use thiserror::Error;

/// The user whose data an operation may access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserContext(UserId);

impl UserContext {
    #[must_use]
    pub const fn new(user_id: UserId) -> Self {
        Self(user_id)
    }

    #[must_use]
    pub const fn user_id(&self) -> UserId {
        self.0
    }
}

/// One row in an activity list.
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityPreview {
    observation_id: ObservationId,
    creator: CreatorDiagnostics,
    summary: ActivitySummary,
}

impl ActivityPreview {
    #[must_use]
    pub const fn observation_id(&self) -> ObservationId {
        self.observation_id
    }

    #[must_use]
    pub const fn creator(&self) -> &CreatorDiagnostics {
        &self.creator
    }

    #[must_use]
    pub const fn summary(&self) -> ActivitySummary {
        self.summary
    }
}

impl From<StoredActivitySummary> for ActivityPreview {
    fn from(stored: StoredActivitySummary) -> Self {
        let observation_id = stored.observation_id();
        let summary = stored.summary();
        Self {
            observation_id,
            creator: stored.into_creator(),
            summary,
        }
    }
}

/// One complete activity returned to a target.
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityDetails {
    observation_id: ObservationId,
    normalized: NormalizedActivity,
}

impl ActivityDetails {
    #[must_use]
    pub const fn observation_id(&self) -> ObservationId {
        self.observation_id
    }

    #[must_use]
    pub const fn normalized(&self) -> &NormalizedActivity {
        &self.normalized
    }
}

impl From<StoredActivity> for ActivityDetails {
    fn from(stored: StoredActivity) -> Self {
        let observation_id = stored.observation_id();
        Self {
            observation_id,
            normalized: stored.into_normalized(),
        }
    }
}

/// Shared in-process application service.
pub struct Application {
    storage: Storage,
}

impl Application {
    #[must_use]
    pub const fn new(storage: Storage) -> Self {
        Self { storage }
    }

    /// Lists profiles in stable display order.
    /// # Errors
    /// [`enum@Error`] when persisted profile data cannot be loaded.
    pub async fn profiles(&self) -> Result<Vec<User>, Error> {
        Ok(self.storage.users().await?)
    }

    /// Creates a portable profile.
    ///
    /// The first profile owns the deployment; later profiles are members.
    /// # Errors
    /// [`enum@Error`] when the profile cannot be stored.
    pub async fn create_profile(&self, display_name: DisplayName) -> Result<User, Error> {
        let role = if self.storage.users().await?.is_empty() {
            Role::Owner
        } else {
            Role::Member
        };
        let user = User::from_parts(
            UserId::new_v4(),
            role,
            Profile::from_display_name(display_name),
        );
        self.storage.save_user(&user).await?;
        Ok(user)
    }

    /// Replaces one profile's presentation preferences.
    /// # Errors
    /// [`enum@Error`] when the profile is missing or cannot be stored.
    pub async fn update_profile_preferences(
        &self,
        user: UserContext,
        preferences: ProfilePreferences,
    ) -> Result<User, Error> {
        let Some(mut stored) = self.storage.user(user.user_id()).await? else {
            return Err(Error::ProfileNotFound(user.user_id()));
        };
        let mut replacement = stored.profile().clone();
        replacement.replace_preferences(preferences);
        stored.replace_profile(replacement);
        self.storage.save_user(&stored).await?;
        Ok(stored)
    }

    /// Loads the selected profile image.
    /// # Errors
    /// [`enum@Error`] when persisted avatar data cannot be loaded.
    pub async fn profile_avatar(&self, user: UserContext) -> Result<Option<ProfileAvatar>, Error> {
        Ok(self
            .storage
            .profile_avatar(user.user_id())
            .await?
            .map(|avatar| {
                let artifact_id = avatar.thumbnail_artifact_id();
                ProfileAvatar {
                    artifact_id,
                    thumbnail: avatar.into_thumbnail_bytes(),
                }
            }))
    }

    /// Stores a profile image and selects its cropped thumbnail.
    /// # Errors
    /// [`enum@Error`] when ownership, image validation, or persistence fails.
    pub async fn import_avatar(
        &self,
        request: AvatarImportRequest<'_>,
    ) -> Result<AvatarImportReceipt, Error> {
        if request.user.user_id() != request.source.owner_id() {
            return Err(garmin_importer::AvatarImportError::ActorCannotUseSource.into());
        }
        self.storage.save_source(request.source).await?;
        Ok(AvatarImporter::new(&self.storage)
            .import(
                ImporterAvatarRequest::from_parts(
                    request.user.user_id(),
                    request.source,
                    request.source_identity,
                    request.operation_id,
                    request.acquired_at,
                    request.bytes,
                )
                .crop(request.crop),
            )
            .await?)
    }

    /// Imports one complete FIT artifact for its owning user.
    ///
    /// The connector source is saved before the immutable acquisition. Parser
    /// rejection remains a successful, preserved import result.
    /// # Errors
    /// [`enum@Error`] for invalid ownership, definitions, or persistence.
    pub async fn import_fit(
        &self,
        request: FitImportRequest<'_>,
    ) -> Result<FitImportResult, Error> {
        if request.user.user_id() != request.source.owner_id() {
            return Err(garmin_importer::ImportError::ActorCannotUseSource.into());
        }
        self.storage.save_source(request.source).await?;
        let previous = self
            .storage
            .acquisition_time(request.user.user_id(), request.operation_id)
            .await?;
        let disposition = if previous.is_some() {
            ImportDisposition::Existing
        } else {
            ImportDisposition::Created
        };
        let outcome = FitImporter::new(&self.storage)
            .import(ImporterRequest::from_parts(
                request.user.user_id(),
                request.source,
                request.source_identity,
                request.operation_id,
                previous.unwrap_or(request.acquired_at),
                request.bytes,
            ))
            .await?;
        Ok(FitImportResult::from_outcome(outcome, disposition))
    }

    /// Inspects GPX bytes without storing them.
    /// # Errors
    /// [`enum@Error`] when the GPX document cannot be parsed.
    pub fn inspect_gpx(bytes: &[u8]) -> Result<garmin_gpx::Document, Error> {
        Ok(garmin_gpx::parse(bytes)?)
    }

    /// Imports one explicitly selected GPX route candidate.
    /// # Errors
    /// [`enum@Error`] for invalid ownership, selection, definitions, or persistence.
    pub async fn import_route(
        &self,
        request: RouteImportRequest<'_>,
    ) -> Result<RouteImportReceipt, Error> {
        if request.user.user_id() != request.source.owner_id() {
            return Err(garmin_importer::RouteImportError::ActorCannotUseSource.into());
        }
        self.storage.save_source(request.source).await?;
        Ok(RouteImporter::new(&self.storage)
            .import(ImporterRouteRequest::from_parts(
                request.user.user_id(),
                request.source,
                request.source_identity,
                request.operation_id,
                request.acquired_at,
                request.bytes,
                request.candidate,
                request.name,
                request.sport,
            ))
            .await?)
    }

    /// Lists the scoped user's route plans, newest first.
    /// # Errors
    /// [`enum@Error`] when persisted route data cannot be loaded.
    pub async fn route_plans(
        &self,
        user: UserContext,
    ) -> Result<Vec<StoredRoutePlanSummary>, Error> {
        Ok(self.storage.route_plans(user.user_id()).await?)
    }

    /// Loads one route plan within the user's scope.
    /// # Errors
    /// [`enum@Error`] when persisted route data cannot be loaded.
    pub async fn route_plan(
        &self,
        user: UserContext,
        plan_id: RoutePlanId,
    ) -> Result<Option<StoredRoutePlan>, Error> {
        Ok(self.storage.route_plan(user.user_id(), plan_id).await?)
    }

    /// Confirms direct lines between an unresolved route's control points.
    /// # Errors
    /// [`enum@Error`] for an absent, exact, short, or concurrently changed route.
    pub async fn confirm_route_straight_lines(
        &self,
        user: UserContext,
        plan_id: RoutePlanId,
        created_at: Timestamp,
    ) -> Result<StoredRoutePlan, Error> {
        let Some(stored) = self.storage.route_plan(user.user_id(), plan_id).await? else {
            return Err(Error::RoutePlanNotFound(plan_id));
        };
        let (plan, source) = stored.into_parts();
        let revision = garmin_route::confirm_straight_lines(
            &source,
            garmin_model::route::RoutePlanRevisionId::new_v4(),
            created_at,
        )?;
        Ok(self
            .storage
            .save_route_revision(user.user_id(), plan, &revision)
            .await?)
    }

    /// Lists the scoped user's activities, newest first.
    /// # Errors
    /// [`enum@Error`] when persisted activity data cannot be loaded.
    pub async fn activities(&self, user: UserContext) -> Result<Vec<ActivityPreview>, Error> {
        Ok(self
            .storage
            .activities(user.user_id())
            .await?
            .into_iter()
            .map(ActivityPreview::from)
            .collect())
    }

    /// Loads one complete activity within the user's scope.
    /// # Errors
    /// [`enum@Error`] when persisted activity data cannot be loaded.
    pub async fn activity(
        &self,
        user: UserContext,
        observation_id: ObservationId,
    ) -> Result<Option<ActivityDetails>, Error> {
        Ok(self
            .storage
            .activity(user.user_id(), observation_id)
            .await?
            .map(ActivityDetails::from))
    }

    /// Closes the deployment store.
    pub async fn close(self) {
        self.storage.close().await;
    }
}

/// Selected profile-image data for presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileAvatar {
    artifact_id: ArtifactId,
    thumbnail: Vec<u8>,
}

impl ProfileAvatar {
    #[must_use]
    pub const fn artifact_id(&self) -> ArtifactId {
        self.artifact_id
    }

    #[must_use]
    pub fn thumbnail(&self) -> &[u8] {
        &self.thumbnail
    }

    #[must_use]
    pub fn into_thumbnail(self) -> Vec<u8> {
        self.thumbnail
    }
}

/// One profile image presented by a connector.
pub struct AvatarImportRequest<'a> {
    user: UserContext,
    source: &'a Source,
    source_identity: SourceIdentity,
    operation_id: AcquisitionOperationId,
    acquired_at: Timestamp,
    bytes: &'a [u8],
    crop: AvatarCrop,
}

impl<'a> AvatarImportRequest<'a> {
    #[must_use]
    pub const fn from_parts(
        user: UserContext,
        source: &'a Source,
        source_identity: SourceIdentity,
        operation_id: AcquisitionOperationId,
        acquired_at: Timestamp,
        bytes: &'a [u8],
        crop: AvatarCrop,
    ) -> Self {
        Self {
            user,
            source,
            source_identity,
            operation_id,
            acquired_at,
            bytes,
            crop,
        }
    }
}

/// One complete FIT artifact presented by a connector.
pub struct FitImportRequest<'a> {
    user: UserContext,
    source: &'a Source,
    source_identity: SourceIdentity,
    operation_id: AcquisitionOperationId,
    acquired_at: Timestamp,
    bytes: &'a [u8],
}

/// One selected GPX candidate presented by a connector.
pub struct RouteImportRequest<'a> {
    user: UserContext,
    source: &'a Source,
    source_identity: SourceIdentity,
    operation_id: AcquisitionOperationId,
    acquired_at: Timestamp,
    bytes: &'a [u8],
    candidate: garmin_gpx::CandidateSource,
    name: RouteName,
    sport: RouteSport,
}

impl<'a> RouteImportRequest<'a> {
    /// Creates a request from validated source data and explicit user choices.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "the request fully defines source, selection, and user choices"
    )]
    pub const fn from_parts(
        user: UserContext,
        source: &'a Source,
        source_identity: SourceIdentity,
        operation_id: AcquisitionOperationId,
        acquired_at: Timestamp,
        bytes: &'a [u8],
        candidate: garmin_gpx::CandidateSource,
        name: RouteName,
        sport: RouteSport,
    ) -> Self {
        Self {
            user,
            source,
            source_identity,
            operation_id,
            acquired_at,
            bytes,
            candidate,
            name,
            sport,
        }
    }
}

impl<'a> FitImportRequest<'a> {
    #[must_use]
    pub const fn from_parts(
        user: UserContext,
        source: &'a Source,
        source_identity: SourceIdentity,
        operation_id: AcquisitionOperationId,
        acquired_at: Timestamp,
        bytes: &'a [u8],
    ) -> Self {
        Self {
            user,
            source,
            source_identity,
            operation_id,
            acquired_at,
            bytes,
        }
    }
}

/// Preserved result of one FIT normalization attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FitImportResult {
    Imported {
        /// Source-ordered activity observations.
        observations: Vec<ObservationId>,
        /// Whether this acquisition was newly stored or already present.
        disposition: ImportDisposition,
    },
    Rejected {
        /// Stored parser failure.
        failure: NormalizationFailure,
        /// Whether this acquisition was newly stored or already present.
        disposition: ImportDisposition,
    },
}

impl FitImportResult {
    #[must_use]
    pub const fn activity_count(&self) -> usize {
        match self {
            Self::Imported { observations, .. } => observations.len(),
            Self::Rejected { .. } => 0,
        }
    }

    #[must_use]
    pub const fn disposition(&self) -> ImportDisposition {
        match self {
            Self::Imported { disposition, .. } | Self::Rejected { disposition, .. } => *disposition,
        }
    }

    fn from_outcome(outcome: FitImportOutcome, disposition: ImportDisposition) -> Self {
        match outcome {
            FitImportOutcome::Imported(receipt) => Self::Imported {
                observations: receipt.observation_ids().to_vec(),
                disposition,
            },
            FitImportOutcome::Rejected { failure, .. } => Self::Rejected {
                failure,
                disposition,
            },
        }
    }
}

/// Persistence effect of one completed import.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportDisposition {
    Created,
    Existing,
}

/// Application query failure.
#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Storage(#[from] garmin_storage::Error),
    #[error("profile {0} does not exist")]
    ProfileNotFound(UserId),
    #[error(transparent)]
    Import(#[from] garmin_importer::ImportError),
    #[error(transparent)]
    AvatarImport(#[from] garmin_importer::AvatarImportError),
    #[error(transparent)]
    Gpx(#[from] garmin_gpx::Error),
    #[error(transparent)]
    RouteImport(#[from] garmin_importer::RouteImportError),
    #[error("route plan {0} does not exist")]
    RoutePlanNotFound(RoutePlanId),
    #[error(transparent)]
    RouteTransformation(#[from] garmin_route::Error),
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use futures_lite::future::block_on;
    use garmin_fit::fixture;
    use garmin_fixtures::seed;
    use garmin_model::{
        artifact::{AcquisitionOperationId, SourceIdentity},
        identity::{Role, Source, SourceId},
        value::Timestamp,
    };
    use tempfile::tempdir;

    use super::*;

    const GPX_FIXTURE: &[u8] = include_bytes!("../../garmin-gpx/tests/fixtures/candidates.gpx");

    #[test]
    fn profiles_and_activity_reads_remain_user_scoped() -> Result<(), Box<dyn Error>> {
        block_on(async {
            let root = tempdir()?;
            let storage = Storage::open(root.path().join("application.sqlite3")).await?;
            let corpus = seed(&storage).await?;
            let application = Application::new(storage);

            assert_eq!(application.profiles().await?, corpus.users());
            for user in corpus.users() {
                let context = UserContext::new(user.id());
                let previews = application.activities(context).await?;
                assert!(previews.iter().all(|preview| {
                    corpus.activities().iter().any(|activity| {
                        activity.owner_id() == user.id()
                            && activity.receipt().observation_ids() == [preview.observation_id()]
                    })
                }));
                for preview in previews {
                    let details = application
                        .activity(context, preview.observation_id())
                        .await?
                        .ok_or("listed activity could not be loaded")?;
                    assert_eq!(details.normalized().activity().summary(), preview.summary());

                    for other in corpus
                        .users()
                        .iter()
                        .filter(|other| other.id() != user.id())
                    {
                        assert_eq!(
                            application
                                .activity(UserContext::new(other.id()), preview.observation_id(),)
                                .await?,
                            None
                        );
                    }
                }
            }

            application.close().await;
            Ok(())
        })
    }

    #[test]
    fn profile_creation_assigns_one_owner_and_allows_duplicate_names() -> Result<(), Box<dyn Error>>
    {
        block_on(async {
            let root = tempdir()?;
            let storage = Storage::open(root.path().join("profiles.sqlite3")).await?;
            let application = Application::new(storage);

            let owner = application.create_profile("Rider".parse()?).await?;
            let member = application.create_profile(" Rider ".parse()?).await?;

            assert_eq!(owner.role(), Role::Owner);
            assert_eq!(member.role(), Role::Member);
            assert_ne!(owner.id(), member.id());
            assert_eq!(owner.profile(), member.profile());
            application.close().await;
            Ok(())
        })
    }

    #[test]
    fn profile_preferences_update_only_the_scoped_profile() -> Result<(), Box<dyn Error>> {
        block_on(async {
            let root = tempdir()?;
            let storage = Storage::open(root.path().join("preferences.sqlite3")).await?;
            let application = Application::new(storage);
            let owner = application.create_profile("Rider".parse()?).await?;
            let other = application.create_profile("Runner".parse()?).await?;
            let preferences = ProfilePreferences::from_parts(
                garmin_model::identity::UnitSystem::Imperial,
                garmin_model::identity::LanguagePreference::Czech,
                garmin_model::identity::ThemePreference::Dark,
            );

            let updated = application
                .update_profile_preferences(UserContext::new(owner.id()), preferences)
                .await?;

            assert_eq!(updated.profile().preferences(), preferences);
            assert_eq!(
                application
                    .profiles()
                    .await?
                    .into_iter()
                    .find(|user| user.id() == other.id())
                    .ok_or("second profile was not returned")?
                    .profile()
                    .preferences(),
                ProfilePreferences::default()
            );
            assert!(matches!(
                application
                    .update_profile_preferences(
                        UserContext::new(UserId::new_v4()),
                        ProfilePreferences::default(),
                    )
                    .await,
                Err(super::Error::ProfileNotFound(_))
            ));
            application.close().await;
            Ok(())
        })
    }

    #[test]
    fn fit_import_saves_its_source_and_refreshes_activity_queries() -> Result<(), Box<dyn Error>> {
        block_on(async {
            let root = tempdir()?;
            let storage = Storage::open(root.path().join("imports.sqlite3")).await?;
            let application = Application::new(storage);
            let user = application.create_profile("Rider".parse()?).await?;
            let source = Source::from_parts(
                SourceId::new_v4(),
                user.id(),
                "Selected files".parse()?,
                None,
            );
            let bytes = fixture::activity(fixture::Sport::Running)?;
            let operation_id = AcquisitionOperationId::new_v4();
            let result = application
                .import_fit(FitImportRequest::from_parts(
                    UserContext::new(user.id()),
                    &source,
                    SourceIdentity::from_string("activity.fit".to_owned())?,
                    operation_id,
                    Timestamp::from_unix_seconds(1_788_198_400)?,
                    &bytes,
                ))
                .await?;

            assert_eq!(result.activity_count(), 1);
            assert_eq!(result.disposition(), ImportDisposition::Created);
            let retry = application
                .import_fit(FitImportRequest::from_parts(
                    UserContext::new(user.id()),
                    &source,
                    SourceIdentity::from_string("activity.fit".to_owned())?,
                    operation_id,
                    Timestamp::from_unix_seconds(1_788_198_500)?,
                    &bytes,
                ))
                .await?;
            assert_eq!(retry.disposition(), ImportDisposition::Existing);
            assert_eq!(
                application
                    .activities(UserContext::new(user.id()))
                    .await?
                    .len(),
                1
            );
            application.close().await;
            Ok(())
        })
    }

    #[test]
    fn rejected_actor_cannot_register_another_users_source() -> Result<(), Box<dyn Error>> {
        block_on(async {
            let root = tempdir()?;
            let storage = Storage::open(root.path().join("ownership.sqlite3")).await?;
            let application = Application::new(storage);
            let actor = application.create_profile("Rider".parse()?).await?;
            let other = application.create_profile("Runner".parse()?).await?;
            let source_id = SourceId::new_v4();
            let foreign =
                Source::from_parts(source_id, other.id(), "Selected files".parse()?, None);
            let bytes = fixture::activity(fixture::Sport::Running)?;

            assert!(matches!(
                application
                    .import_fit(FitImportRequest::from_parts(
                        UserContext::new(actor.id()),
                        &foreign,
                        "activity.fit".parse()?,
                        AcquisitionOperationId::new_v4(),
                        Timestamp::from_unix_seconds(1_788_198_400)?,
                        &bytes,
                    ))
                    .await,
                Err(super::Error::Import(
                    garmin_importer::ImportError::ActorCannotUseSource
                ))
            ));

            let owned = Source::from_parts(source_id, actor.id(), "Selected files".parse()?, None);
            application
                .import_fit(FitImportRequest::from_parts(
                    UserContext::new(actor.id()),
                    &owned,
                    "activity.fit".parse()?,
                    AcquisitionOperationId::new_v4(),
                    Timestamp::from_unix_seconds(1_788_198_400)?,
                    &bytes,
                ))
                .await?;
            application.close().await;
            Ok(())
        })
    }

    async fn assert_direct_route_round_trip(
        application: &Application,
        owner: UserId,
        other: UserId,
        source: &Source,
        document: &garmin_gpx::Document,
    ) -> Result<(), Box<dyn Error>> {
        let control_points = document
            .candidates()
            .iter()
            .find(|candidate| {
                matches!(
                    candidate.source(),
                    garmin_gpx::CandidateSource::Route { .. }
                )
            })
            .ok_or("fixture contains no control-point route")?;
        let unresolved = application
            .import_route(RouteImportRequest::from_parts(
                UserContext::new(owner),
                source,
                "morning-loop.gpx".parse()?,
                AcquisitionOperationId::from_u128(43),
                Timestamp::from_unix_seconds(1_788_198_400)?,
                GPX_FIXTURE,
                control_points.source(),
                "Direct route".parse()?,
                garmin_model::route::RouteSport::Cycling,
            ))
            .await?;
        let confirmed = application
            .confirm_route_straight_lines(
                UserContext::new(owner),
                unresolved.plan_id(),
                Timestamp::from_unix_seconds(1_788_198_500)?,
            )
            .await?;
        assert!(confirmed.revision().shape().is_geometry());
        assert_eq!(
            confirmed.revision().provenance().source(),
            garmin_model::route::RevisionSource::Revision(unresolved.revision_id())
        );
        let course = garmin_fit::course::encode(
            confirmed.revision(),
            garmin_fit::course::SerialNumber::from_u32(1)?,
        )?;
        let decoded = garmin_fit::course::decode(&course)?;
        assert_eq!(decoded.name(), confirmed.revision().name());
        assert_eq!(
            decoded.points().len(),
            confirmed.revision().shape().points().len()
        );
        assert!(matches!(
            application
                .confirm_route_straight_lines(
                    UserContext::new(other),
                    unresolved.plan_id(),
                    Timestamp::from_unix_seconds(1_788_198_500)?,
                )
                .await,
            Err(super::Error::RoutePlanNotFound(_))
        ));
        Ok(())
    }

    #[test]
    fn selected_gpx_candidates_round_trip_with_user_scope() -> Result<(), Box<dyn Error>> {
        block_on(async {
            let root = tempdir()?;
            let storage = Storage::open(root.path().join("routes.sqlite3")).await?;
            let application = Application::new(storage);
            let owner = application.create_profile("Rider".parse()?).await?;
            let other = application.create_profile("Runner".parse()?).await?;
            let source = Source::from_parts(
                SourceId::new_v4(),
                owner.id(),
                "Selected files".parse()?,
                None,
            );
            let document = Application::inspect_gpx(GPX_FIXTURE)?;
            let candidate = document
                .candidates()
                .first()
                .ok_or("fixture contains no route candidate")?;
            let receipt = application
                .import_route(RouteImportRequest::from_parts(
                    UserContext::new(owner.id()),
                    &source,
                    "morning-loop.gpx".parse()?,
                    AcquisitionOperationId::from_u128(42),
                    Timestamp::from_unix_seconds(1_788_198_400)?,
                    GPX_FIXTURE,
                    candidate.source(),
                    "Morning loop".parse()?,
                    garmin_model::route::RouteSport::Cycling,
                ))
                .await?;

            let summaries = application
                .route_plans(UserContext::new(owner.id()))
                .await?;
            assert_eq!(summaries.len(), 1);
            assert_eq!(summaries[0].plan().id(), receipt.plan_id());
            assert_eq!(summaries[0].point_count(), 2);
            let stored = application
                .route_plan(UserContext::new(owner.id()), receipt.plan_id())
                .await?
                .ok_or("imported route was not returned")?;
            assert_eq!(stored.revision().shape(), candidate.shape());
            assert_eq!(
                application
                    .route_plan(UserContext::new(other.id()), receipt.plan_id())
                    .await?,
                None
            );
            assert_direct_route_round_trip(
                &application,
                owner.id(),
                other.id(),
                &source,
                &document,
            )
            .await?;
            application.close().await;
            Ok(())
        })
    }
}
