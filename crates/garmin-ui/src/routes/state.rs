//! Client view state; host-issued identities survive retries and UI redraws.

use garmin_model::{
    artifact::ArtifactId,
    route::{RouteCandidateSource, RoutePoint, RouteSport},
};
use garmin_service_api::routes::{
    CourseVersion, GpxCandidate, GpxRejected, GpxUpload, GpxUploadPhase, MAX_GEOMETRY_CHUNK,
    RouteReply, RouteRequest, RouteSelection, RouteSource, RouteSummary,
};

#[cfg(test)]
mod tests;

#[derive(Default)]
pub struct State {
    pub routes: Vec<RouteSummary>,
    pub next: Option<u32>,
    pub upload: Option<GpxUpload>,
    pub candidates: Vec<GpxCandidate>,
    pub rejected: Vec<GpxRejected>,
    pub candidate: Option<RouteCandidateSource>,
    pub name: String,
    pub sport: Option<RouteSport>,
    pub detail: Option<RouteSummary>,
    pub source: Option<RouteSource>,
    pub versions: Vec<CourseVersion>,
    pub next_versions: Option<u32>,
    pub points: Vec<RoutePoint>,
    pub points_ready: bool,
    pub busy: bool,
    pub error: Option<String>,
    pub retry: Option<RouteRequest>,
    pub pending: Option<RouteRequest>,
}

pub enum Action {
    Import,
    Request(RouteRequest),
    Download(ArtifactId),
}

impl State {
    pub fn queue(&mut self, request: RouteRequest) {
        self.error = None;
        self.retry = None;
        self.pending = Some(request);
    }

    pub fn take_request(&mut self) -> Option<RouteRequest> {
        if self.busy {
            return None;
        }
        let request = self.pending.take()?;
        self.busy = true;
        self.retry = Some(request.clone());
        Some(request)
    }

    pub fn fail(&mut self, message: String) {
        self.busy = false;
        self.pending = None;
        self.error = Some(message);
    }

    pub fn select_candidate(&mut self, source: RouteCandidateSource) {
        self.candidate = Some(source);
        self.points.clear();
        self.points_ready = false;
        self.name = self
            .candidates
            .iter()
            .find(|candidate| candidate.source == source)
            .and_then(|candidate| candidate.suggested_name.as_ref())
            .map(ToString::to_string)
            .unwrap_or_default();
        if let Some(upload) = &self.upload {
            self.queue(RouteRequest::PreviewPoints {
                operation: upload.operation,
                candidate: source,
                offset: 0,
                count: MAX_GEOMETRY_CHUNK,
            });
        }
    }

    #[must_use]
    pub fn confirm(&self) -> Option<RouteRequest> {
        let upload = self.upload.as_ref()?;
        let GpxUploadPhase::Review { digest, .. } = upload.phase else {
            return None;
        };
        if !self.points_ready || self.name.len() > 4096 {
            return None;
        }
        Some(RouteRequest::Confirm {
            operation: upload.operation,
            selection: RouteSelection {
                digest,
                candidate: self.candidate?,
                name: self.name.parse().ok()?,
                sport: self.sport?,
            },
        })
    }

    pub fn accept(&mut self, reply: RouteReply) {
        let queued = self.pending.take();
        let append_versions =
            matches!(self.retry, Some(RouteRequest::Versions { offset, .. }) if offset > 0);
        let append_routes = matches!(self.retry, Some(RouteRequest::List { offset }) if offset > 0);
        self.busy = false;
        self.retry = None;
        self.error = None;
        match reply {
            RouteReply::Routes { items, next } => {
                self.accept_routes(items, append_routes);
                self.next = next;
                self.detail = None;
            }
            RouteReply::Detail { route, source } => {
                let revision = route.revision;
                self.detail = Some(route);
                self.source = source;
                self.points.clear();
                self.points_ready = false;
                self.versions.clear();
                self.pending = Some(RouteRequest::RevisionPoints {
                    revision,
                    offset: 0,
                    count: MAX_GEOMETRY_CHUNK,
                });
            }
            RouteReply::Points {
                offset,
                points,
                end,
            } => self.accept_points(offset, points, end),
            RouteReply::Versions { items, next } => {
                self.accept_versions(items, append_versions);
                self.next_versions = next;
            }
            RouteReply::Upload(upload) => self.accept_upload(upload),
            RouteReply::Candidates {
                items,
                rejected,
                next,
            } => {
                self.candidates.extend(items);
                self.rejected.extend(rejected);
                if let (Some(offset), Some(upload)) = (next, &self.upload) {
                    self.pending = Some(RouteRequest::Candidates {
                        operation: upload.operation,
                        offset,
                    });
                }
            }
            RouteReply::Imported(receipt) => {
                self.sport = None;
                self.name.clear();
                self.upload = None;
                self.candidate = None;
                self.candidates.clear();
                self.rejected.clear();
                self.points.clear();
                self.pending = Some(RouteRequest::Detail {
                    plan: receipt.plan_id(),
                });
            }
            RouteReply::Cancelled => {
                self.sport = None;
                self.name.clear();
                self.upload = None;
                self.candidate = None;
                self.candidates.clear();
                self.rejected.clear();
                self.points.clear();
                self.pending = Some(RouteRequest::List { offset: 0 });
            }
            RouteReply::GenerationReady(operation) => {
                if let Some(route) = &self.detail {
                    self.pending = Some(RouteRequest::Generate {
                        operation,
                        revision: route.revision,
                    });
                }
            }
            RouteReply::Generated(course) => {
                self.versions.clear();
                self.pending = Some(RouteRequest::Versions {
                    revision: course.revision,
                    offset: 0,
                });
            }
            RouteReply::CourseDeleted { revision } => {
                self.versions.clear();
                self.pending = Some(RouteRequest::Versions {
                    revision,
                    offset: 0,
                });
            }
        }
        if self.error.is_none() {
            self.pending = queued.or(self.pending.take());
        }
    }

    fn accept_routes(&mut self, items: Vec<RouteSummary>, append: bool) {
        if !append {
            self.routes.clear();
        }
        for item in items {
            if !self.routes.iter().any(|existing| existing.id == item.id) {
                self.routes.push(item);
            }
        }
    }

    fn accept_versions(&mut self, items: Vec<CourseVersion>, append: bool) {
        if !append {
            self.versions.clear();
        }
        for item in items {
            if !self.versions.iter().any(|existing| existing.id == item.id) {
                self.versions.push(item);
            }
        }
    }

    fn accept_upload(&mut self, upload: GpxUpload) {
        match &upload.phase {
            GpxUploadPhase::Parsing => {
                self.pending = Some(RouteRequest::UploadStatus {
                    operation: upload.operation,
                });
            }
            GpxUploadPhase::Review { .. }
                if self.candidates.is_empty() && self.rejected.is_empty() =>
            {
                self.pending = Some(RouteRequest::Candidates {
                    operation: upload.operation,
                    offset: 0,
                });
            }
            GpxUploadPhase::Failed(error) => {
                self.error = Some(error.clone());
                self.retry = Some(RouteRequest::Inspect {
                    operation: upload.operation,
                });
            }
            GpxUploadPhase::Uploading | GpxUploadPhase::Review { .. } => {}
        }
        self.upload = Some(upload);
    }

    fn accept_points(&mut self, offset: u32, points: Vec<RoutePoint>, end: bool) {
        if (!end && points.is_empty())
            || offset as usize != self.points.len()
            || self.points.len().saturating_add(points.len()) > 1_000_000
        {
            self.fail("invalid route geometry response".into());
            return;
        }
        self.points.extend(points);
        let offset = u32::try_from(self.points.len()).unwrap_or(u32::MAX);
        self.points_ready = end;
        if let (Some(upload), Some(candidate)) = (&self.upload, self.candidate) {
            if !end {
                self.pending = Some(RouteRequest::PreviewPoints {
                    operation: upload.operation,
                    candidate,
                    offset,
                    count: MAX_GEOMETRY_CHUNK,
                });
            }
        } else if let Some(route) = &self.detail {
            self.pending = Some(if end {
                RouteRequest::Versions {
                    revision: route.revision,
                    offset: 0,
                }
            } else {
                RouteRequest::RevisionPoints {
                    revision: route.revision,
                    offset,
                    count: MAX_GEOMETRY_CHUNK,
                }
            });
        }
    }
}
