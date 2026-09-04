SELECT
    (SELECT count(*) FROM artifact_blobs) AS blob_count,
    (
        SELECT count(*)
        FROM normalization_runs
        WHERE outcome = 'failed'
    ) AS failed_count,
    (SELECT count(*) FROM observations) AS observation_count;
