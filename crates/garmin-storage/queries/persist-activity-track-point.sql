INSERT INTO activity_track_points (
    observation_id,
    position,
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
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT DO UPDATE SET observation_id = excluded.observation_id
WHERE activity_track_points.timestamp_ms = excluded.timestamp_ms
AND activity_track_points.latitude_degrees IS excluded.latitude_degrees
AND activity_track_points.longitude_degrees IS excluded.longitude_degrees
AND activity_track_points.elevation_m IS excluded.elevation_m
AND activity_track_points.distance_mm IS excluded.distance_mm
AND activity_track_points.speed_mm_s IS excluded.speed_mm_s
AND activity_track_points.heart_rate_bpm IS excluded.heart_rate_bpm
AND activity_track_points.cadence_rpm IS excluded.cadence_rpm
AND activity_track_points.power_w IS excluded.power_w
AND activity_track_points.temperature_millicelsius
IS excluded.temperature_millicelsius
RETURNING observation_id;
