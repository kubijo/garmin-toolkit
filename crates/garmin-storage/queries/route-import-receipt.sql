SELECT
    acquisitions.id AS acquisition_id,
    acquisitions.artifact_id,
    acquisitions.owner_id,
    acquisitions.source_id,
    acquisitions.source_identity,
    acquisitions.acquired_at,
    artifacts.digest,
    route_import_receipts.plan_id,
    route_import_receipts.revision_id,
    route_import_receipts.candidate_kind,
    route_import_receipts.candidate_index,
    route_import_receipts.segment_index,
    route_import_receipts.parser_name,
    route_import_receipts.parser_version,
    route_plan_revisions.name,
    route_plan_revisions.sport
FROM route_import_receipts
INNER JOIN acquisitions
    ON route_import_receipts.acquisition_id = acquisitions.id
INNER JOIN artifacts ON acquisitions.artifact_id = artifacts.id
INNER JOIN route_plan_revisions
    ON route_import_receipts.revision_id = route_plan_revisions.id
WHERE acquisitions.owner_id = ? AND acquisitions.operation_id = ?;
