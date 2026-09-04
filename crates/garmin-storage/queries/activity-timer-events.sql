SELECT
    timestamp_ms,
    state
FROM activity_timer_events
WHERE observation_id = ?
ORDER BY position;
