SELECT count(*) AS observation_count
FROM observations
WHERE normalization_run_id = ?;
