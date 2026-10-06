use garmin_model::{
    artifact::{ArtifactDigest, SourceIdentity},
    identity::SourceId,
    route::{RouteCandidateSource, RouteImportIdentity},
};
use garmin_service_api::routes::{
    GpxCandidate, GpxRejected, MAX_UPLOAD_CHUNK, PAGE_SIZE, RouteSelection,
};

use super::{
    AcquisitionOperationId, Application, Arc, ByteCount, Confirmation, Instant, Kind, MAX_UPLOADS,
    PreparedGpx, Result, RouteReply, RouteSession, Source, Stage, TTL, Upload, Uuid, failure,
    internal, reads,
};
use crate::RouteImportRequest;

impl RouteSession {
    pub(super) fn start(&self, file_name: String, size: ByteCount) -> Result<RouteReply> {
        let total = usize::try_from(size.as_u64())
            .map_err(|_| failure(Kind::Limit, "GPX file is too large"))?;
        if total == 0 || total > garmin_gpx::MAX_BYTES {
            return Err(failure(
                Kind::Limit,
                "GPX file must be between 1 byte and 16 MiB",
            ));
        }
        if file_name.trim().is_empty()
            || file_name.len() > 255
            || file_name
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\'))
        {
            return Err(failure(
                Kind::InvalidInput,
                "select a GPX file with a valid filename",
            ));
        }
        let mut pending = self.operations.pending();
        if pending.uploads.len() >= MAX_UPLOADS {
            return Err(failure(Kind::Limit, "too many GPX uploads are pending"));
        }
        let source_name = format!("garmin-toolkit/gpx-source/{}", self.actor.user_id());
        let source_id = SourceId::from_u128(
            Uuid::new_v5(&Uuid::NAMESPACE_URL, source_name.as_bytes()).as_u128(),
        );
        let source = Source::from_parts(
            source_id,
            self.actor.user_id(),
            "GPX files".parse().map_err(internal)?,
            None,
        );
        let operation = AcquisitionOperationId::new_v4();
        let entry = Upload {
            actor: self.actor.user_id(),
            epoch: self.epoch,
            expires: Instant::now() + TTL,
            file_name,
            source,
            total,
            confirmations: 0,
            stage: Stage::Uploading(Vec::new()),
        };
        let snapshot = entry.snapshot(operation);
        pending.uploads.insert(operation, entry);
        Ok(RouteReply::Upload(snapshot))
    }

    pub(super) fn append(
        &self,
        operation: AcquisitionOperationId,
        offset: u64,
        bytes: &[u8],
    ) -> Result<RouteReply> {
        if bytes.is_empty() || bytes.len() > MAX_UPLOAD_CHUNK {
            return Err(failure(Kind::Limit, "invalid upload chunk size"));
        }
        let mut pending = self.operations.pending();
        let entry = self.entry(&mut pending, operation)?;
        let Stage::Uploading(stored) = &mut entry.stage else {
            return Err(failure(
                Kind::InvalidState,
                "upload is no longer accepting bytes",
            ));
        };
        let offset = usize::try_from(offset)
            .map_err(|_| failure(Kind::InvalidInput, "invalid upload offset"))?;
        let end = offset
            .checked_add(bytes.len())
            .filter(|end| *end <= entry.total)
            .ok_or_else(|| failure(Kind::Limit, "chunk exceeds the declared file size"))?;
        if offset == stored.len() {
            stored.extend_from_slice(bytes);
        } else if stored.get(offset..end) != Some(bytes) {
            return Err(failure(
                Kind::Conflict,
                "upload chunk conflicts with received bytes",
            ));
        }
        Ok(RouteReply::Upload(entry.snapshot(operation)))
    }

    pub(super) fn status(&self, operation: AcquisitionOperationId) -> Result<RouteReply> {
        let mut pending = self.operations.pending();
        Ok(RouteReply::Upload(
            self.entry(&mut pending, operation)?.snapshot(operation),
        ))
    }

    pub(super) fn inspect(&self, operation: AcquisitionOperationId) -> Result<RouteReply> {
        let mut pending = self.operations.pending();
        let entry = self.entry(&mut pending, operation)?;
        let bytes: Arc<[u8]> = match &entry.stage {
            Stage::Uploading(bytes) if bytes.len() == entry.total => Arc::from(bytes.as_slice()),
            Stage::Failed { bytes, .. } => Arc::clone(bytes),
            Stage::Review(_) | Stage::Parsing { .. } => {
                return Ok(RouteReply::Upload(entry.snapshot(operation)));
            }
            Stage::Uploading(_) => {
                return Err(failure(
                    Kind::InvalidState,
                    "finish the upload before inspection",
                ));
            }
        };
        let input = Arc::clone(&bytes);
        let operations = Arc::downgrade(&self.operations);
        let parser = self.operations.parser.clone();
        let actor = self.actor.user_id();
        let epoch = self.epoch;
        let task = tokio::spawn(async move {
            let result = parser.parse(input).await;
            let Some(operations) = operations.upgrade() else {
                return;
            };
            let mut pending = operations.pending();
            let Some(entry) = pending.uploads.get_mut(&operation) else {
                return;
            };
            if entry.actor != actor
                || entry.epoch != epoch
                || operations.deployment.epoch() != epoch
            {
                return;
            }
            let Stage::Parsing { bytes, .. } = &entry.stage else {
                return;
            };
            entry.stage = match result {
                Ok(prepared) => Stage::Review(prepared),
                Err(error) => Stage::Failed {
                    bytes: Arc::clone(bytes),
                    message: error.to_string(),
                },
            };
        });
        entry.stage = Stage::Parsing {
            bytes,
            task: task.abort_handle(),
        };
        Ok(RouteReply::Upload(entry.snapshot(operation)))
    }

    pub(super) fn candidates(
        &self,
        operation: AcquisitionOperationId,
        offset: u32,
    ) -> Result<RouteReply> {
        let prepared = self.prepared(operation)?.0;
        let document = prepared.document();
        let start = offset as usize;
        let end = start
            .saturating_add(PAGE_SIZE as usize)
            .min(document.candidates().len());
        let items = document
            .candidates()
            .get(start..end)
            .unwrap_or_default()
            .iter()
            .map(|candidate| {
                Ok(GpxCandidate {
                    source: candidate.source(),
                    suggested_name: candidate.suggested_name().cloned(),
                    geometry: candidate.shape().is_geometry(),
                    point_count: u32::try_from(candidate.shape().points().len())
                        .map_err(internal)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        // Rejections are independently bounded by the parser's candidate cap.
        let rejected = if offset == 0 {
            document
                .rejected()
                .iter()
                .map(|candidate| GpxRejected {
                    source: candidate.source(),
                    reason: candidate.reason(),
                })
                .collect()
        } else {
            Vec::new()
        };
        let next = (end < document.candidates().len())
            .then(|| u32::try_from(end).map_err(internal))
            .transpose()?;
        Ok(RouteReply::Candidates {
            items,
            rejected,
            next,
        })
    }

    pub(super) fn preview_points(
        &self,
        operation: AcquisitionOperationId,
        candidate: RouteCandidateSource,
        offset: u32,
        count: u32,
    ) -> Result<RouteReply> {
        let prepared = self.prepared(operation)?.0;
        let candidate = prepared
            .document()
            .candidates()
            .iter()
            .find(|value| value.source() == candidate)
            .ok_or_else(|| failure(Kind::NotFound, "candidate was not found"))?;
        reads::points(candidate.shape().points(), offset, count)
    }

    fn prepared(
        &self,
        operation: AcquisitionOperationId,
    ) -> Result<(PreparedGpx, Source, SourceIdentity)> {
        let mut pending = self.operations.pending();
        let entry = self.entry(&mut pending, operation)?;
        let Stage::Review(prepared) = &entry.stage else {
            return Err(failure(Kind::InvalidState, "GPX preview is not ready"));
        };
        Ok((
            prepared.clone(),
            entry.source.clone(),
            entry.file_name.parse().map_err(internal)?,
        ))
    }

    pub(super) async fn confirm(
        &self,
        app: &Application,
        operation: AcquisitionOperationId,
        selection: RouteSelection,
    ) -> Result<RouteReply> {
        if let Some(previous) = app
            .route_import(self.actor, operation)
            .await
            .map_err(internal)?
        {
            let identity = previous.identity();
            let requested = RouteImportIdentity::from_parts(
                self.actor.user_id(),
                identity.source_id(),
                identity.source_identity().clone(),
                selection.digest,
                selection.candidate,
                selection.name,
                selection.sport,
            );
            let receipt = previous
                .retry(&requested)
                .map_err(|error| failure(Kind::Conflict, error))?;
            self.operations.pending().uploads.remove(&operation);
            return Ok(RouteReply::Imported(receipt));
        }
        let (prepared, source, source_identity) = self.prepared(operation)?;
        {
            let mut pending = self.operations.pending();
            let entry = self.entry(&mut pending, operation)?;
            if entry.confirmations >= 4 {
                return Err(failure(Kind::Limit, "confirmation is already running"));
            }
            entry.confirmations += 1;
        }
        let _confirmation = Confirmation {
            operations: Arc::clone(&self.operations),
            operation,
        };
        if ArtifactDigest::from_bytes(prepared.bytes()) != selection.digest {
            return Err(failure(
                Kind::Conflict,
                "the GPX selection belongs to different bytes",
            ));
        }
        if selection.name.as_str().len() > garmin_gpx::worker::MAX_NAME_BYTES {
            return Err(failure(Kind::Limit, "route name is too long"));
        }
        let request = RouteImportRequest::from_parts(
            self.actor,
            &source,
            source_identity,
            operation,
            reads::now()?,
            prepared.bytes(),
            selection.candidate,
            selection.name,
            selection.sport,
        );
        let receipt = app
            .import_prepared_route(request, &prepared)
            .await
            .map_err(|error| failure(Kind::Conflict, error))?;
        self.operations.pending().uploads.remove(&operation);
        Ok(RouteReply::Imported(receipt))
    }

    pub(super) fn cancel(&self, operation: AcquisitionOperationId) -> Result<RouteReply> {
        let mut pending = self.operations.pending();
        if self.entry(&mut pending, operation)?.confirmations > 0 {
            return Err(failure(
                Kind::InvalidState,
                "confirmed import is being committed",
            ));
        }
        pending.uploads.remove(&operation);
        Ok(RouteReply::Cancelled)
    }
}
