SELECT
    point_position,
    instruction AS cue_text
FROM route_plan_cues
WHERE revision_id = ?
ORDER BY position
