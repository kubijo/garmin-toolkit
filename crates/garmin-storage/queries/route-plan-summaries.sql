SELECT
    route_plans.id AS plan_id,
    route_plans.owner_id,
    route_plan_heads.revision_id,
    route_plan_revisions.created_at,
    route_plan_revisions.name,
    route_plan_revisions.sport,
    route_plan_revisions.shape,
    COUNT(route_plan_points.position) AS point_count
FROM route_plans
INNER JOIN route_plan_heads
    ON route_plans.id = route_plan_heads.plan_id
INNER JOIN route_plan_revisions
    ON
        route_plan_heads.revision_id = route_plan_revisions.id
        AND route_plans.id = route_plan_revisions.plan_id
INNER JOIN route_plan_points
    ON route_plan_revisions.id = route_plan_points.revision_id
WHERE route_plans.owner_id = ?
GROUP BY route_plans.id, route_plan_heads.revision_id
ORDER BY route_plan_revisions.created_at DESC, route_plans.id DESC
