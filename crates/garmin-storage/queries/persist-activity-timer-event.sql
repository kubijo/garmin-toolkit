INSERT INTO activity_timer_events (
    observation_id,
    position,
    timestamp_ms,
    state
)
VALUES (?, ?, ?, ?)
ON CONFLICT DO UPDATE SET observation_id = excluded.observation_id
WHERE activity_timer_events.timestamp_ms = excluded.timestamp_ms
AND activity_timer_events.state = excluded.state
RETURNING observation_id;
