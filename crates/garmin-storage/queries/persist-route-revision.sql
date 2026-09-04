INSERT INTO route_plan_revisions (
    id,
    plan_id,
    previous_revision_id,
    created_at,
    name,
    sport,
    shape,
    source_kind,
    source_artifact_id,
    source_revision_id
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT (id) DO UPDATE SET id = excluded.id
WHERE route_plan_revisions.plan_id = excluded.plan_id
AND route_plan_revisions.previous_revision_id
IS excluded.previous_revision_id
AND route_plan_revisions.created_at = excluded.created_at
AND route_plan_revisions.name = excluded.name
AND route_plan_revisions.sport = excluded.sport
AND route_plan_revisions.shape = excluded.shape
AND route_plan_revisions.source_kind = excluded.source_kind
AND route_plan_revisions.source_artifact_id IS excluded.source_artifact_id
AND route_plan_revisions.source_revision_id IS excluded.source_revision_id
RETURNING id
