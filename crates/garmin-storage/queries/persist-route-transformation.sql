INSERT INTO route_plan_transformations (
    revision_id,
    position,
    component_name,
    component_version
)
VALUES (?, ?, ?, ?)
ON CONFLICT (revision_id, position) DO UPDATE SET
    revision_id = excluded.revision_id
WHERE route_plan_transformations.component_name = excluded.component_name
AND route_plan_transformations.component_version
= excluded.component_version
RETURNING position
