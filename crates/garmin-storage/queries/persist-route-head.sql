INSERT INTO route_plan_heads (plan_id, revision_id)
VALUES (?, ?)
ON CONFLICT (plan_id) DO UPDATE SET plan_id = excluded.plan_id
WHERE route_plan_heads.revision_id = excluded.revision_id
RETURNING plan_id
