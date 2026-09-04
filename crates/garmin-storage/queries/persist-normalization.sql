INSERT INTO normalization_runs (
    id,
    acquisition_id,
    artifact_id,
    owner_id,
    parser_name,
    parser_version,
    schema_version,
    outcome,
    failure
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT DO UPDATE SET id = excluded.id
WHERE normalization_runs.acquisition_id = excluded.acquisition_id
AND normalization_runs.artifact_id = excluded.artifact_id
AND normalization_runs.owner_id = excluded.owner_id
AND normalization_runs.parser_name = excluded.parser_name
AND normalization_runs.parser_version = excluded.parser_version
AND normalization_runs.schema_version = excluded.schema_version
AND normalization_runs.outcome = excluded.outcome
AND normalization_runs.failure IS excluded.failure
RETURNING id;
