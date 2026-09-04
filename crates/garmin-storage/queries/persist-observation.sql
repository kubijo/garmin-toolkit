INSERT INTO observations (
    id,
    owner_id,
    normalization_run_id,
    normalization_outcome,
    kind,
    observed_at,
    fingerprint_schema_name,
    fingerprint_schema_version,
    fingerprint_digest
)
VALUES (?, ?, ?, 'succeeded', ?, ?, ?, ?, ?)
ON CONFLICT DO UPDATE SET id = excluded.id
WHERE observations.owner_id = excluded.owner_id
AND observations.normalization_run_id = excluded.normalization_run_id
AND observations.normalization_outcome = excluded.normalization_outcome
AND observations.kind = excluded.kind
AND observations.observed_at IS excluded.observed_at
AND observations.fingerprint_schema_name = excluded.fingerprint_schema_name
AND observations.fingerprint_schema_version
= excluded.fingerprint_schema_version
AND observations.fingerprint_digest = excluded.fingerprint_digest
RETURNING id;
