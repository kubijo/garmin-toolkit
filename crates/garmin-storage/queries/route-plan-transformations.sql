SELECT
    component_name,
    component_version
FROM route_plan_transformations
WHERE revision_id = ?
ORDER BY position
