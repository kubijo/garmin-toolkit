INSERT INTO route_plan_cues (revision_id, position, point_position, instruction)
VALUES (?, ?, ?, ?)
ON CONFLICT (revision_id, position) DO UPDATE SET
    revision_id = excluded.revision_id
WHERE route_plan_cues.point_position = excluded.point_position
AND route_plan_cues.instruction = excluded.instruction
RETURNING position
