SELECT
    activity_projections.observation_id,
    observations.owner_id,
    observations.fingerprint_schema_name,
    observations.fingerprint_schema_version,
    observations.fingerprint_digest,
    activity_projections.normalization_run_id,
    activity_projections.sequence_position,
    activity_projections.sport,
    activity_projections.start_ms,
    activity_projections.end_ms,
    activity_projections.elapsed_ms,
    activity_projections.timer_ms,
    activity_projections.distance_mm,
    activity_projections.energy_kcal,
    activity_projections.ascent_mm,
    activity_projections.descent_mm,
    activity_projections.average_speed_mm_s,
    activity_projections.maximum_speed_mm_s,
    activity_projections.average_heart_rate_bpm,
    activity_projections.maximum_heart_rate_bpm,
    activity_projections.average_cadence_rpm,
    activity_projections.maximum_cadence_rpm,
    activity_projections.average_power_w,
    activity_projections.maximum_power_w,
    fit_creator_diagnostics.manufacturer_id,
    fit_creator_diagnostics.product_id,
    fit_creator_diagnostics.serial_number,
    fit_creator_diagnostics.product_name,
    fit_creator_diagnostics.software_version_hundredths
FROM activity_projections
INNER JOIN observations
    ON activity_projections.observation_id = observations.id
LEFT OUTER JOIN fit_creator_diagnostics
    ON
        activity_projections.observation_id
        = fit_creator_diagnostics.observation_id
WHERE observations.owner_id = ?
ORDER BY
    activity_projections.start_ms DESC,
    activity_projections.observation_id ASC;
