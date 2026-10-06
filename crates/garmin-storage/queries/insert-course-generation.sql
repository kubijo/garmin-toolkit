INSERT INTO course_generations (
    id, operation_id, owner_id, plan_id, revision_id, artifact_id,
    version, serial, encoder_name, encoder_version, generated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);
