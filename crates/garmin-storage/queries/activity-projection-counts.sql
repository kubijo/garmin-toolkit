SELECT
    (
        SELECT count(*)
        FROM activity_laps
        WHERE observation_id = ?
    ) AS lap_count,
    (
        SELECT count(*)
        FROM fit_creator_diagnostics
        WHERE observation_id = ?
    ) AS creator_count,
    (
        SELECT count(*)
        FROM activity_track_points
        WHERE observation_id = ?
    ) AS track_point_count,
    (
        SELECT count(*)
        FROM activity_timer_events
        WHERE observation_id = ?
    ) AS timer_event_count;
