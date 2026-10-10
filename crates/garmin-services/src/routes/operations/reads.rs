use std::time::{SystemTime, UNIX_EPOCH};

use garmin_model::{
    route::{RoutePlanId, RoutePoint},
    value::Timestamp,
};
use garmin_service_api::routes::{CourseVersion, MAX_GEOMETRY_CHUNK, PAGE_SIZE, RouteSummary};
use garmin_storage::{GeneratedCourse, StoredRoutePlanSummary};

use super::{
    Application, CourseGenerationOperationId, Generation, Instant, Kind, MAX_GENERATIONS, Result,
    RoutePlanRevisionId, RouteReply, RouteSession, TTL, failure, internal,
};

impl RouteSession {
    pub(super) async fn list(&self, app: &Application, offset: u32) -> Result<RouteReply> {
        let routes = app
            .storage
            .route_plans_page(self.actor.user_id(), offset, PAGE_SIZE + 1)
            .await
            .map_err(internal)?;
        let (items, next) = page(routes, offset);
        let items = items.iter().map(summary).collect::<Result<Vec<_>>>()?;
        Ok(RouteReply::Routes { items, next })
    }

    pub(super) async fn detail(&self, app: &Application, plan: RoutePlanId) -> Result<RouteReply> {
        let stored = app
            .storage
            .route_plan_summary(self.actor.user_id(), plan)
            .await
            .map_err(internal)?
            .ok_or_else(|| failure(Kind::NotFound, "route was not found"))?;
        let source = app
            .storage
            .route_source(self.actor.user_id(), plan)
            .await
            .map_err(internal)?;
        Ok(RouteReply::Detail {
            route: summary(&stored)?,
            source: source.map(|source| garmin_service_api::routes::RouteSource {
                artifact: source.artifact,
                digest: source.digest,
                byte_count: source.byte_count,
                name: source.name,
            }),
        })
    }

    pub(super) async fn revision_points(
        &self,
        app: &Application,
        revision: RoutePlanRevisionId,
        offset: u32,
        count: u32,
    ) -> Result<RouteReply> {
        if count == 0 || count > MAX_GEOMETRY_CHUNK {
            return Err(failure(Kind::Limit, "invalid geometry chunk size"));
        }
        let mut points = app
            .storage
            .route_points_page(self.actor.user_id(), revision, offset, count + 1)
            .await
            .map_err(internal)?;
        let end = points.len() <= count as usize;
        points.truncate(count as usize);
        Ok(RouteReply::Points {
            offset,
            points,
            end,
        })
    }

    pub(super) async fn versions(
        &self,
        app: &Application,
        revision: RoutePlanRevisionId,
        offset: u32,
    ) -> Result<RouteReply> {
        let encoder = crate::routes::encoder().map_err(internal)?;
        let versions = app
            .storage
            .generated_courses_page(
                self.actor.user_id(),
                revision,
                &encoder,
                offset,
                PAGE_SIZE + 1,
            )
            .await
            .map_err(internal)?;
        let (items, next) = page(versions, offset);
        Ok(RouteReply::Versions {
            items: items.iter().map(course).collect(),
            next,
        })
    }

    pub(super) async fn prepare_generation(
        &self,
        app: &Application,
        revision: RoutePlanRevisionId,
    ) -> Result<RouteReply> {
        let stored = app
            .route_revision(self.actor, revision)
            .await
            .map_err(internal)?
            .ok_or_else(|| failure(Kind::NotFound, "route revision was not found"))?;
        if !stored.shape().is_geometry() {
            return Err(failure(
                Kind::InvalidState,
                "this route contains unresolved control points",
            ));
        }
        let mut pending = self.operations.pending();
        if pending.generations.len() >= MAX_GENERATIONS {
            return Err(failure(
                Kind::Limit,
                "too many Course generations are pending",
            ));
        }
        let operation = CourseGenerationOperationId::new_v4();
        pending.generations.insert(
            operation,
            Generation {
                actor: self.actor.user_id(),
                epoch: self.epoch,
                expires: Instant::now() + TTL,
                revision,
            },
        );
        Ok(RouteReply::GenerationReady(operation))
    }

    pub(super) async fn generate(
        &self,
        app: &Application,
        operation: CourseGenerationOperationId,
        revision: RoutePlanRevisionId,
    ) -> Result<RouteReply> {
        if let Some(record) = app
            .storage
            .course_generation(self.actor.user_id(), operation)
            .await
            .map_err(internal)?
        {
            if record.revision_id() != revision {
                return Err(failure(
                    Kind::Conflict,
                    "generation operation belongs to a different revision",
                ));
            }
            self.operations.pending().generations.remove(&operation);
            return Ok(RouteReply::Generated(course(&record)));
        }
        {
            let pending = self.operations.pending();
            let entry = pending
                .generations
                .get(&operation)
                .filter(|entry| entry.actor == self.actor.user_id() && entry.epoch == self.epoch)
                .ok_or_else(|| {
                    failure(
                        Kind::NotFound,
                        "generation approval was not found or has expired",
                    )
                })?;
            if entry.revision != revision {
                return Err(failure(
                    Kind::Conflict,
                    "generation operation belongs to a different revision",
                ));
            }
        }
        let record = app
            .generate_course(self.actor, operation, revision, now()?)
            .await
            .map_err(internal)?;
        self.operations.pending().generations.remove(&operation);
        Ok(RouteReply::Generated(course(&record)))
    }
}

fn summary(stored: &StoredRoutePlanSummary) -> Result<RouteSummary> {
    Ok(RouteSummary {
        id: stored.plan().id(),
        revision: stored.plan().current_revision_id(),
        name: stored.name().clone(),
        sport: stored.sport(),
        created_at: stored.created_at(),
        geometry: stored.is_geometry(),
        point_count: u32::try_from(stored.point_count()).map_err(internal)?,
    })
}

pub(super) fn course(record: &GeneratedCourse) -> CourseVersion {
    CourseVersion {
        id: record.id(),
        operation: record.operation_id(),
        revision: record.revision_id(),
        artifact: record.artifact_id(),
        version: record.version(),
        serial: record.serial().into_nonzero(),
        encoder: record.encoder().clone(),
        current_encoder: record.encoder().name() == garmin_fit::course::ENCODER_NAME
            && record.encoder().version().to_string() == garmin_fit::course::ENCODER_VERSION,
        generated_at: record.generated_at(),
        byte_count: record.byte_count(),
        digest: record.digest(),
    }
}

pub(super) fn page<T>(items: Vec<T>, offset: u32) -> (Vec<T>, Option<u32>) {
    let next = if items.len() > PAGE_SIZE as usize {
        offset.checked_add(PAGE_SIZE)
    } else {
        None
    };
    (items.into_iter().take(PAGE_SIZE as usize).collect(), next)
}

pub(super) fn points(points: &[RoutePoint], offset: u32, count: u32) -> Result<RouteReply> {
    if count == 0 || count > MAX_GEOMETRY_CHUNK {
        return Err(failure(Kind::Limit, "invalid geometry chunk size"));
    }
    let start = offset as usize;
    if start > points.len() {
        return Err(failure(Kind::InvalidInput, "invalid geometry offset"));
    }
    let end = start.saturating_add(count as usize).min(points.len());
    Ok(RouteReply::Points {
        offset,
        points: points[start..end].to_vec(),
        end: end == points.len(),
    })
}

pub(super) fn now() -> Result<Timestamp> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(internal)?
        .as_millis();
    Timestamp::from_unix_milliseconds(i64::try_from(millis).map_err(internal)?).map_err(internal)
}
