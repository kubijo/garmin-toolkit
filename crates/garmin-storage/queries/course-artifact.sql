SELECT
    artifact_blobs.bytes,
    artifact_blobs.byte_count,
    artifacts.digest
FROM course_generations
INNER JOIN route_plans ON course_generations.plan_id = route_plans.id
INNER JOIN artifacts ON course_generations.artifact_id = artifacts.id
INNER JOIN artifact_blobs ON artifacts.digest = artifact_blobs.digest
WHERE route_plans.owner_id = ? AND course_generations.artifact_id = ?;
