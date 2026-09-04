SELECT
    route_plans.id AS plan_id,
    route_plans.owner_id,
    route_plan_heads.revision_id,
    route_plan_revisions.previous_revision_id,
    route_plan_revisions.created_at,
    route_plan_revisions.name,
    route_plan_revisions.sport,
    route_plan_revisions.shape,
    route_plan_revisions.source_kind,
    route_plan_revisions.source_artifact_id,
    route_plan_revisions.source_revision_id
FROM route_plans
INNER JOIN route_plan_heads
    ON route_plans.id = route_plan_heads.plan_id
INNER JOIN route_plan_revisions
    ON
        route_plan_heads.revision_id = route_plan_revisions.id
        AND route_plans.id = route_plan_revisions.plan_id
WHERE route_plans.owner_id = ? AND route_plans.id = ?
