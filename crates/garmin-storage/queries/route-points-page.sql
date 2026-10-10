SELECT
    route_plan_points.latitude_degrees,
    route_plan_points.longitude_degrees,
    route_plan_points.elevation_m
FROM route_plan_points
INNER JOIN route_plan_revisions
    ON route_plan_points.revision_id = route_plan_revisions.id
INNER JOIN route_plans ON route_plan_revisions.plan_id = route_plans.id
WHERE
    route_plans.owner_id = ? AND route_plan_points.revision_id = ?
    AND route_plan_points.position >= ?
ORDER BY route_plan_points.position
LIMIT ?;
