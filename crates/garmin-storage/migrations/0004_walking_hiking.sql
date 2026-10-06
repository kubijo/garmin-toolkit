-- Rebuild the two sport-constrained tables atomically. Save dependent rows
-- before DROP TABLE applies their ON DELETE CASCADE actions.
PRAGMA defer_foreign_keys = ON;

-- Heads do not cascade, so remove their references before replacing revisions.
CREATE TEMP TABLE saved_route_plan_heads AS
SELECT
    plan_id,
    revision_id
FROM route_plan_heads;
DELETE FROM route_plan_heads;

CREATE TEMP TABLE saved_fit_creator_diagnostics AS
SELECT
    observation_id,
    manufacturer_id,
    product_id,
    serial_number,
    product_name,
    software_version_hundredths
FROM fit_creator_diagnostics;

CREATE TEMP TABLE saved_activity_laps AS
SELECT
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
FROM activity_laps;

CREATE TEMP TABLE saved_activity_track_points AS
SELECT
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
FROM activity_track_points;

CREATE TEMP TABLE saved_activity_timer_events AS
SELECT
    observation_id,
    position,
    timestamp_ms,
    state
FROM activity_timer_events;

CREATE TEMP TABLE saved_route_plan_points AS
SELECT
    revision_id,
    position,
    latitude_degrees,
    longitude_degrees,
    elevation_m
FROM route_plan_points;

CREATE TEMP TABLE saved_route_plan_cues AS
SELECT
    revision_id,
    position,
    point_position,
    instruction
FROM route_plan_cues;

CREATE TEMP TABLE saved_route_plan_transformations AS
SELECT
    revision_id,
    position,
    component_name,
    component_version
FROM route_plan_transformations;

CREATE TABLE new_activity_projections (
    observation_id TEXT PRIMARY KEY NOT NULL,
    normalization_run_id TEXT NOT NULL,
    observation_kind TEXT NOT NULL DEFAULT 'activity'
    CHECK (observation_kind = 'activity'),
    sequence_position INTEGER NOT NULL CHECK (sequence_position >= 0),
    sport TEXT NOT NULL CHECK (
        sport IN ('running', 'cycling', 'swimming', 'walking', 'hiking')
    ),
    start_ms INTEGER NOT NULL,
    end_ms INTEGER NOT NULL CHECK (end_ms >= start_ms),
    elapsed_ms INTEGER NOT NULL CHECK (elapsed_ms >= 0),
    timer_ms INTEGER NOT NULL CHECK (
        timer_ms >= 0 AND timer_ms <= elapsed_ms
    ),
    distance_mm INTEGER CHECK (distance_mm >= 0),
    energy_kcal INTEGER CHECK (energy_kcal >= 0),
    ascent_mm INTEGER CHECK (ascent_mm >= 0),
    descent_mm INTEGER CHECK (descent_mm >= 0),
    average_speed_mm_s INTEGER CHECK (average_speed_mm_s >= 0),
    maximum_speed_mm_s INTEGER CHECK (maximum_speed_mm_s >= 0),
    average_heart_rate_bpm INTEGER CHECK (average_heart_rate_bpm >= 0),
    maximum_heart_rate_bpm INTEGER CHECK (maximum_heart_rate_bpm >= 0),
    average_cadence_rpm REAL CHECK (average_cadence_rpm >= 0),
    maximum_cadence_rpm REAL CHECK (maximum_cadence_rpm >= 0),
    average_power_w INTEGER CHECK (average_power_w >= 0),
    maximum_power_w INTEGER CHECK (maximum_power_w >= 0),
    UNIQUE (normalization_run_id, sequence_position),
    FOREIGN KEY (observation_id, normalization_run_id, observation_kind)
    REFERENCES observations (id, normalization_run_id, kind)
    ON DELETE CASCADE
) STRICT;

INSERT INTO new_activity_projections (
    observation_id,
    normalization_run_id,
    observation_kind,
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
SELECT
    observation_id,
    normalization_run_id,
    observation_kind,
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
FROM activity_projections;
DROP TABLE activity_projections;
ALTER TABLE new_activity_projections RENAME TO activity_projections;

CREATE TABLE new_route_plan_revisions (
    id TEXT PRIMARY KEY NOT NULL,
    plan_id TEXT NOT NULL REFERENCES route_plans (id) ON DELETE CASCADE,
    previous_revision_id TEXT,
    created_at TEXT NOT NULL,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    sport TEXT NOT NULL CHECK (
        sport IN ('running', 'cycling', 'walking', 'hiking')
    ),
    shape TEXT NOT NULL CHECK (shape IN ('geometry', 'control_points')),
    source_kind TEXT NOT NULL CHECK (
        source_kind IN ('freehand', 'artifact', 'revision')
    ),
    source_artifact_id TEXT REFERENCES artifacts (id),
    source_revision_id TEXT REFERENCES new_route_plan_revisions (id),
    CHECK (
        (
            source_kind = 'freehand'
            AND source_artifact_id IS NULL
            AND source_revision_id IS NULL
        )
        OR (
            source_kind = 'artifact'
            AND source_artifact_id IS NOT NULL
            AND source_revision_id IS NULL
        )
        OR (
            source_kind = 'revision'
            AND source_artifact_id IS NULL
            AND source_revision_id IS NOT NULL
        )
    ),
    UNIQUE (id, plan_id),
    FOREIGN KEY (previous_revision_id, plan_id)
    REFERENCES new_route_plan_revisions (id, plan_id)
) STRICT;

INSERT INTO new_route_plan_revisions (
    id,
    plan_id,
    previous_revision_id,
    created_at,
    name,
    sport,
    shape,
    source_kind,
    source_artifact_id,
    source_revision_id
)
SELECT
    id,
    plan_id,
    previous_revision_id,
    created_at,
    name,
    sport,
    shape,
    source_kind,
    source_artifact_id,
    source_revision_id
FROM route_plan_revisions;
DROP TABLE route_plan_revisions;
ALTER TABLE new_route_plan_revisions RENAME TO route_plan_revisions;

INSERT INTO fit_creator_diagnostics (
    observation_id,
    manufacturer_id,
    product_id,
    serial_number,
    product_name,
    software_version_hundredths
)
SELECT
    observation_id,
    manufacturer_id,
    product_id,
    serial_number,
    product_name,
    software_version_hundredths
FROM saved_fit_creator_diagnostics;
DROP TABLE saved_fit_creator_diagnostics;

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
SELECT
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
FROM saved_activity_laps;
DROP TABLE saved_activity_laps;

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
SELECT
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
FROM saved_activity_track_points;
DROP TABLE saved_activity_track_points;

INSERT INTO activity_timer_events (
    observation_id, position, timestamp_ms, state
)
SELECT
    observation_id,
    position,
    timestamp_ms,
    state
FROM saved_activity_timer_events;
DROP TABLE saved_activity_timer_events;

INSERT INTO route_plan_points (
    revision_id, position, latitude_degrees, longitude_degrees, elevation_m
)
SELECT
    revision_id,
    position,
    latitude_degrees,
    longitude_degrees,
    elevation_m
FROM saved_route_plan_points;
DROP TABLE saved_route_plan_points;

INSERT INTO route_plan_cues (revision_id, position, point_position, instruction)
SELECT
    revision_id,
    position,
    point_position,
    instruction
FROM saved_route_plan_cues;
DROP TABLE saved_route_plan_cues;

INSERT INTO route_plan_transformations (
    revision_id, position, component_name, component_version
)
SELECT
    revision_id,
    position,
    component_name,
    component_version
FROM saved_route_plan_transformations;
DROP TABLE saved_route_plan_transformations;

INSERT INTO route_plan_heads (plan_id, revision_id)
SELECT
    plan_id,
    revision_id
FROM saved_route_plan_heads;
DROP TABLE saved_route_plan_heads;

CREATE INDEX activity_projections_start_idx
ON activity_projections (start_ms DESC, observation_id);

CREATE INDEX route_plan_revisions_plan_id_idx
ON route_plan_revisions (plan_id, created_at DESC);
