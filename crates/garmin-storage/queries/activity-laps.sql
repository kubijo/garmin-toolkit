SELECT
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
FROM activity_laps
WHERE observation_id = ?
ORDER BY position;
