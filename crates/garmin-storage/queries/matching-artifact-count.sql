SELECT count(DISTINCT normalization_runs.artifact_id) AS artifact_count
FROM observations
INNER JOIN normalization_runs
    ON observations.normalization_run_id = normalization_runs.id
WHERE
    observations.owner_id = ?
    AND observations.kind = ?
    AND observations.fingerprint_schema_name = ?
    AND observations.fingerprint_schema_version = ?
    AND observations.fingerprint_digest = ?;
