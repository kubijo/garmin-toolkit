SELECT
    artifact_blobs.bytes,
    artifact_blobs.byte_count,
    artifacts.digest
FROM artifacts
INNER JOIN artifact_blobs ON artifacts.digest = artifact_blobs.digest
WHERE
    artifacts.id = ?
    AND EXISTS (
        SELECT 1
        FROM route_plan_revisions
        INNER JOIN route_plans ON route_plan_revisions.plan_id = route_plans.id
        WHERE
            route_plans.owner_id = ?
            AND route_plan_revisions.source_artifact_id = artifacts.id
    );
