SELECT
    artifacts.id,
    artifacts.digest,
    artifact_blobs.byte_count,
    acquisitions.source_identity
FROM route_plan_revisions
INNER JOIN route_plans ON route_plan_revisions.plan_id = route_plans.id
INNER JOIN artifacts ON route_plan_revisions.source_artifact_id = artifacts.id
INNER JOIN artifact_blobs ON artifacts.digest = artifact_blobs.digest
INNER JOIN acquisitions
    ON
        artifacts.id = acquisitions.artifact_id
        AND route_plans.owner_id = acquisitions.owner_id
WHERE
    route_plans.owner_id = ? AND route_plans.id = ?
    AND route_plan_revisions.previous_revision_id IS NULL;
