INSERT INTO normalization_transformations (
    normalization_run_id,
    position,
    component_name,
    component_version
)
VALUES (?, ?, ?, ?)
ON CONFLICT DO UPDATE SET normalization_run_id = excluded.normalization_run_id
WHERE normalization_transformations.component_name = excluded.component_name
AND normalization_transformations.component_version = excluded.component_version
RETURNING normalization_run_id;
