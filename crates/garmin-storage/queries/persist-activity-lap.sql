INSERT INTO activity_laps (
    observation_id,
    position,
    start_ms,
    end_ms,
    elapsed_ms,
    timer_ms,
    distance_mm,
    energy_kcal,
    ascent_mm,
    descent_mm,
    average_speed_mm_s,
    maximum_speed_mm_s,
    average_heart_rate_bpm,
    maximum_heart_rate_bpm,
    average_cadence_rpm,
    maximum_cadence_rpm,
    average_power_w,
    maximum_power_w
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT DO UPDATE SET observation_id = excluded.observation_id
WHERE activity_laps.start_ms = excluded.start_ms
AND activity_laps.end_ms = excluded.end_ms
AND activity_laps.elapsed_ms = excluded.elapsed_ms
AND activity_laps.timer_ms = excluded.timer_ms
AND activity_laps.distance_mm IS excluded.distance_mm
AND activity_laps.energy_kcal IS excluded.energy_kcal
AND activity_laps.ascent_mm IS excluded.ascent_mm
AND activity_laps.descent_mm IS excluded.descent_mm
AND activity_laps.average_speed_mm_s IS excluded.average_speed_mm_s
AND activity_laps.maximum_speed_mm_s IS excluded.maximum_speed_mm_s
AND activity_laps.average_heart_rate_bpm IS excluded.average_heart_rate_bpm
AND activity_laps.maximum_heart_rate_bpm IS excluded.maximum_heart_rate_bpm
AND activity_laps.average_cadence_rpm IS excluded.average_cadence_rpm
AND activity_laps.maximum_cadence_rpm IS excluded.maximum_cadence_rpm
AND activity_laps.average_power_w IS excluded.average_power_w
AND activity_laps.maximum_power_w IS excluded.maximum_power_w
RETURNING observation_id;
