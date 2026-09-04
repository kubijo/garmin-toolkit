INSERT INTO route_plans (id, owner_id)
VALUES (?, ?)
ON CONFLICT (id) DO UPDATE SET id = excluded.id
WHERE route_plans.owner_id = excluded.owner_id
RETURNING id
