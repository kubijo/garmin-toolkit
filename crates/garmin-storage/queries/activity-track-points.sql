SELECT
    timestamp_ms,
    latitude_degrees,
    longitude_degrees,
    elevation_m,
    distance_mm,
    speed_mm_s,
    heart_rate_bpm,
    cadence_rpm,
    power_w,
    temperature_millicelsius
FROM activity_track_points
WHERE observation_id = ?
ORDER BY position;
