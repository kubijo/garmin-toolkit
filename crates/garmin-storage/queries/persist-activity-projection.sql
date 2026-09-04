INSERT INTO activity_projections (
    observation_id,
    normalization_run_id,
    sequence_position,
    sport,
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
VALUES (
    ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
)
ON CONFLICT DO UPDATE SET observation_id = activity_projections.observation_id
WHERE activity_projections.observation_id = excluded.observation_id
AND activity_projections.normalization_run_id = excluded.normalization_run_id
AND activity_projections.sequence_position = excluded.sequence_position
AND activity_projections.sport = excluded.sport
AND activity_projections.start_ms = excluded.start_ms
AND activity_projections.end_ms = excluded.end_ms
AND activity_projections.elapsed_ms = excluded.elapsed_ms
AND activity_projections.timer_ms = excluded.timer_ms
AND activity_projections.distance_mm IS excluded.distance_mm
AND activity_projections.energy_kcal IS excluded.energy_kcal
AND activity_projections.ascent_mm IS excluded.ascent_mm
AND activity_projections.descent_mm IS excluded.descent_mm
AND activity_projections.average_speed_mm_s IS excluded.average_speed_mm_s
AND activity_projections.maximum_speed_mm_s IS excluded.maximum_speed_mm_s
AND activity_projections.average_heart_rate_bpm
IS excluded.average_heart_rate_bpm
AND activity_projections.maximum_heart_rate_bpm
IS excluded.maximum_heart_rate_bpm
AND activity_projections.average_cadence_rpm IS excluded.average_cadence_rpm
AND activity_projections.maximum_cadence_rpm IS excluded.maximum_cadence_rpm
AND activity_projections.average_power_w IS excluded.average_power_w
AND activity_projections.maximum_power_w IS excluded.maximum_power_w
RETURNING observation_id;
