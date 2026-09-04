UPDATE route_plan_heads
SET revision_id = ?
WHERE
    plan_id = ?
    AND revision_id = ?
    AND EXISTS (
        SELECT 1
        FROM route_plans
        WHERE
            route_plans.id = route_plan_heads.plan_id
            AND route_plans.owner_id = ?
    )
RETURNING plan_id
