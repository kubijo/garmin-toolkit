DELETE FROM course_generations
WHERE id = ? AND owner_id = ? AND revision_id = ?
RETURNING operation_id, version, artifact_id;
