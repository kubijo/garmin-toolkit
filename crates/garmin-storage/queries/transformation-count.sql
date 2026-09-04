SELECT count(*) AS transformation_count
FROM normalization_transformations
WHERE normalization_run_id = ?;
