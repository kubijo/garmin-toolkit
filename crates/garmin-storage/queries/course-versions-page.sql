SELECT
    course_generations.id,
    course_generations.operation_id,
    course_generations.owner_id,
    course_generations.plan_id,
    course_generations.revision_id,
    course_generations.artifact_id,
    course_generations.version,
    course_generations.serial,
    course_generations.encoder_name,
    course_generations.encoder_version,
    course_generations.generated_at,
    artifact_blobs.byte_count,
    artifacts.digest
FROM course_generations
INNER JOIN route_plans ON course_generations.plan_id = route_plans.id
INNER JOIN artifacts ON course_generations.artifact_id = artifacts.id
INNER JOIN artifact_blobs ON artifacts.digest = artifact_blobs.digest
WHERE route_plans.owner_id = ? AND course_generations.revision_id = ?
ORDER BY
    course_generations.id = (
        SELECT matching.id FROM course_generations AS matching
        WHERE
            matching.revision_id = course_generations.revision_id
            AND matching.encoder_name = ? AND matching.encoder_version = ?
        ORDER BY matching.version DESC
        LIMIT 1
    ) DESC,
    course_generations.version DESC
LIMIT ? OFFSET ?;
