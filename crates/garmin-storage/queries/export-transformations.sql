SELECT
    position,
    component_name,
    component_version
FROM normalization_transformations
WHERE normalization_run_id = ?
ORDER BY position;
