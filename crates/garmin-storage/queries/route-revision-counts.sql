SELECT
    (
        SELECT COUNT(*)
        FROM route_plan_points
        WHERE revision_id = ?
    ) AS point_count,
    (
        SELECT COUNT(*)
        FROM route_plan_cues
        WHERE revision_id = ?
    ) AS cue_count,
    (
        SELECT COUNT(*)
        FROM route_plan_transformations
        WHERE revision_id = ?
    ) AS transformation_count
