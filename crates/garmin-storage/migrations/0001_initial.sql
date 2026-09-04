CREATE TABLE users (
    id TEXT PRIMARY KEY NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('owner', 'member')),
    display_name TEXT NOT NULL CHECK (length(trim(display_name)) > 0),
    accent_rgba INTEGER CHECK (accent_rgba BETWEEN 0 AND 4294967295),
    avatar_artifact_id TEXT REFERENCES artifacts (id) ON DELETE SET NULL,
    unit_system TEXT NOT NULL DEFAULT 'metric'
    CHECK (unit_system IN ('metric', 'imperial')),
    language TEXT NOT NULL DEFAULT 'en' CHECK (language IN ('en', 'cs')),
    theme TEXT NOT NULL DEFAULT 'auto'
    CHECK (theme IN ('auto', 'dark', 'light'))
) STRICT;

CREATE TABLE devices (
    id TEXT PRIMARY KEY NOT NULL,
    label TEXT NOT NULL CHECK (length(trim(label)) > 0)
) STRICT;

CREATE TABLE sources (
    id TEXT PRIMARY KEY NOT NULL,
    owner_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    label TEXT NOT NULL CHECK (length(trim(label)) > 0),
    device_id TEXT REFERENCES devices (id) ON DELETE SET NULL,
    UNIQUE (id, owner_id)
) STRICT;

CREATE TABLE artifact_blobs (
    digest BLOB PRIMARY KEY NOT NULL CHECK (length(digest) = 32),
    byte_count INTEGER NOT NULL CHECK (byte_count >= 0),
    bytes BLOB NOT NULL,
    CHECK (byte_count = length(bytes))
) STRICT;

CREATE TABLE artifacts (
    id TEXT PRIMARY KEY NOT NULL,
    digest BLOB NOT NULL REFERENCES artifact_blobs (digest),
    media_type TEXT NOT NULL CHECK (length(trim(media_type)) > 0)
) STRICT;

CREATE TABLE acquisitions (
    id TEXT PRIMARY KEY NOT NULL,
    operation_id TEXT NOT NULL UNIQUE,
    artifact_id TEXT NOT NULL REFERENCES artifacts (id),
    owner_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    source_id TEXT NOT NULL,
    source_identity TEXT NOT NULL CHECK (length(trim(source_identity)) > 0),
    acquired_at TEXT NOT NULL,
    UNIQUE (id, artifact_id, owner_id),
    FOREIGN KEY (source_id, owner_id) REFERENCES sources (id, owner_id)
) STRICT;

CREATE TABLE normalization_runs (
    id TEXT PRIMARY KEY NOT NULL,
    acquisition_id TEXT NOT NULL,
    artifact_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    parser_name TEXT NOT NULL CHECK (length(trim(parser_name)) > 0),
    parser_version TEXT NOT NULL,
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    outcome TEXT NOT NULL CHECK (outcome IN ('succeeded', 'failed')),
    failure TEXT,
    CHECK (
        (outcome = 'succeeded' AND failure IS NULL)
        OR (outcome = 'failed' AND length(trim(failure)) > 0)
    ),
    UNIQUE (id, owner_id, outcome),
    FOREIGN KEY (acquisition_id, artifact_id, owner_id)
    REFERENCES acquisitions (id, artifact_id, owner_id)
) STRICT;

CREATE TABLE normalization_transformations (
    normalization_run_id TEXT NOT NULL REFERENCES normalization_runs (
        id
    ) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    component_name TEXT NOT NULL CHECK (length(trim(component_name)) > 0),
    component_version TEXT NOT NULL,
    PRIMARY KEY (normalization_run_id, position)
) STRICT;

CREATE TABLE observations (
    id TEXT PRIMARY KEY NOT NULL,
    owner_id TEXT NOT NULL,
    normalization_run_id TEXT NOT NULL,
    normalization_outcome TEXT NOT NULL DEFAULT 'succeeded'
    CHECK (normalization_outcome = 'succeeded'),
    kind TEXT NOT NULL CHECK (kind IN ('activity', 'measurement', 'device')),
    observed_at TEXT,
    fingerprint_schema_name TEXT NOT NULL CHECK (
        length(trim(fingerprint_schema_name)) > 0
    ),
    fingerprint_schema_version TEXT NOT NULL,
    fingerprint_digest BLOB NOT NULL CHECK (length(fingerprint_digest) = 32),
    UNIQUE (id, owner_id, kind),
    FOREIGN KEY (normalization_run_id, owner_id, normalization_outcome)
    REFERENCES normalization_runs (id, owner_id, outcome)
) STRICT;

CREATE TABLE association_groups (
    id TEXT PRIMARY KEY NOT NULL,
    owner_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('activity', 'measurement', 'device')),
    basis TEXT NOT NULL CHECK (
        basis IN ('equal_fingerprint', 'user_confirmed')
    ),
    fingerprint_schema_name TEXT,
    fingerprint_schema_version TEXT,
    fingerprint_digest BLOB CHECK (length(fingerprint_digest) = 32),
    CHECK (
        (
            basis = 'equal_fingerprint'
            AND fingerprint_schema_name IS NOT NULL
            AND fingerprint_schema_version IS NOT NULL
            AND fingerprint_digest IS NOT NULL
        )
        OR (
            basis = 'user_confirmed'
            AND fingerprint_schema_name IS NULL
            AND fingerprint_schema_version IS NULL
            AND fingerprint_digest IS NULL
        )
    ),
    UNIQUE (id, owner_id, kind)
) STRICT;

CREATE TABLE association_group_members (
    association_group_id TEXT NOT NULL,
    observation_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    PRIMARY KEY (association_group_id, observation_id),
    FOREIGN KEY (association_group_id, owner_id, kind)
    REFERENCES association_groups (id, owner_id, kind) ON DELETE CASCADE,
    FOREIGN KEY (observation_id, owner_id, kind)
    REFERENCES observations (id, owner_id, kind) ON DELETE CASCADE
) STRICT;

CREATE UNIQUE INDEX observations_projection_identity_idx
ON observations (id, normalization_run_id, kind);

CREATE TABLE activity_projections (
    observation_id TEXT PRIMARY KEY NOT NULL,
    normalization_run_id TEXT NOT NULL,
    observation_kind TEXT NOT NULL DEFAULT 'activity'
    CHECK (observation_kind = 'activity'),
    sequence_position INTEGER NOT NULL CHECK (sequence_position >= 0),
    sport TEXT NOT NULL CHECK (sport IN ('running', 'cycling')),
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

CREATE TABLE fit_creator_diagnostics (
    observation_id TEXT PRIMARY KEY NOT NULL REFERENCES activity_projections (
        observation_id
    ) ON DELETE CASCADE,
    manufacturer_id INTEGER NOT NULL CHECK (
        manufacturer_id BETWEEN 0 AND 65534
    ),
    product_id INTEGER CHECK (product_id BETWEEN 0 AND 65534),
    serial_number INTEGER CHECK (serial_number BETWEEN 1 AND 4294967295),
    product_name TEXT CHECK (length(trim(product_name)) > 0),
    software_version_hundredths INTEGER CHECK (
        software_version_hundredths BETWEEN 0 AND 65534
    )
) STRICT;

CREATE TABLE activity_laps (
    observation_id TEXT NOT NULL REFERENCES activity_projections (
        observation_id
    ) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
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
    PRIMARY KEY (observation_id, position)
) STRICT;

CREATE TABLE activity_track_points (
    observation_id TEXT NOT NULL REFERENCES activity_projections (
        observation_id
    ) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    timestamp_ms INTEGER NOT NULL,
    latitude_degrees REAL CHECK (
        latitude_degrees BETWEEN -90.0 AND 90.0
    ),
    longitude_degrees REAL CHECK (
        longitude_degrees BETWEEN -180.0 AND 180.0
    ),
    elevation_m REAL,
    distance_mm INTEGER CHECK (distance_mm >= 0),
    speed_mm_s INTEGER CHECK (speed_mm_s >= 0),
    heart_rate_bpm INTEGER CHECK (heart_rate_bpm >= 0),
    cadence_rpm REAL CHECK (cadence_rpm >= 0),
    power_w INTEGER CHECK (power_w >= 0),
    temperature_millicelsius INTEGER,
    CHECK (
        (latitude_degrees IS NULL AND longitude_degrees IS NULL)
        OR (latitude_degrees IS NOT NULL AND longitude_degrees IS NOT NULL)
    ),
    PRIMARY KEY (observation_id, position)
) STRICT;

CREATE TABLE activity_timer_events (
    observation_id TEXT NOT NULL REFERENCES activity_projections (
        observation_id
    ) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    timestamp_ms INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('running', 'stopped')),
    PRIMARY KEY (observation_id, position)
) STRICT;

CREATE TABLE profile_avatars (
    owner_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    acquisition_id TEXT NOT NULL,
    original_artifact_id TEXT NOT NULL,
    thumbnail_artifact_id TEXT NOT NULL REFERENCES artifacts (id),
    original_width INTEGER NOT NULL CHECK (original_width > 0),
    original_height INTEGER NOT NULL CHECK (original_height > 0),
    PRIMARY KEY (owner_id, original_artifact_id),
    FOREIGN KEY (acquisition_id, original_artifact_id, owner_id)
    REFERENCES acquisitions (id, artifact_id, owner_id)
) STRICT;

CREATE TABLE route_plans (
    id TEXT PRIMARY KEY NOT NULL,
    owner_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    UNIQUE (id, owner_id)
) STRICT;

CREATE TABLE route_plan_revisions (
    id TEXT PRIMARY KEY NOT NULL,
    plan_id TEXT NOT NULL REFERENCES route_plans (id) ON DELETE CASCADE,
    previous_revision_id TEXT,
    created_at TEXT NOT NULL,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    sport TEXT NOT NULL CHECK (sport IN ('running', 'cycling')),
    shape TEXT NOT NULL CHECK (shape IN ('geometry', 'control_points')),
    source_kind TEXT NOT NULL CHECK (
        source_kind IN ('freehand', 'artifact', 'revision')
    ),
    source_artifact_id TEXT REFERENCES artifacts (id),
    source_revision_id TEXT REFERENCES route_plan_revisions (id),
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
    REFERENCES route_plan_revisions (id, plan_id)
) STRICT;

CREATE TABLE route_plan_heads (
    plan_id TEXT PRIMARY KEY NOT NULL REFERENCES route_plans (id)
    ON DELETE CASCADE,
    revision_id TEXT NOT NULL,
    FOREIGN KEY (revision_id, plan_id)
    REFERENCES route_plan_revisions (id, plan_id)
) STRICT;

CREATE TABLE route_plan_points (
    revision_id TEXT NOT NULL REFERENCES route_plan_revisions (id)
    ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    latitude_degrees REAL NOT NULL CHECK (
        latitude_degrees BETWEEN -90.0 AND 90.0
    ),
    longitude_degrees REAL NOT NULL CHECK (
        longitude_degrees BETWEEN -180.0 AND 180.0
    ),
    elevation_m REAL,
    PRIMARY KEY (revision_id, position)
) STRICT;

CREATE TABLE route_plan_cues (
    revision_id TEXT NOT NULL REFERENCES route_plan_revisions (id)
    ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    point_position INTEGER NOT NULL CHECK (point_position >= 0),
    instruction TEXT NOT NULL CHECK (length(trim(instruction)) > 0),
    PRIMARY KEY (revision_id, position),
    FOREIGN KEY (revision_id, point_position)
    REFERENCES route_plan_points (revision_id, position)
) STRICT;

CREATE TABLE route_plan_transformations (
    revision_id TEXT NOT NULL REFERENCES route_plan_revisions (id)
    ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    component_name TEXT NOT NULL CHECK (length(trim(component_name)) > 0),
    component_version TEXT NOT NULL,
    PRIMARY KEY (revision_id, position)
) STRICT;

CREATE INDEX sources_owner_id_idx ON sources (owner_id);
CREATE INDEX sources_device_id_idx ON sources (device_id);
CREATE INDEX acquisitions_artifact_id_idx ON acquisitions (artifact_id);
CREATE INDEX acquisitions_owner_source_idx ON acquisitions (
    owner_id, source_id
);
CREATE INDEX normalization_runs_acquisition_id_idx
ON normalization_runs (acquisition_id);
CREATE INDEX observations_normalization_run_id_idx
ON observations (normalization_run_id);
CREATE INDEX observations_fingerprint_idx ON observations (
    owner_id,
    kind,
    fingerprint_schema_name,
    fingerprint_schema_version,
    fingerprint_digest
);
CREATE UNIQUE INDEX association_groups_equal_fingerprint_idx
ON association_groups (
    owner_id,
    kind,
    fingerprint_schema_name,
    fingerprint_schema_version,
    fingerprint_digest
)
WHERE basis = 'equal_fingerprint';
CREATE INDEX association_groups_owner_id_idx ON association_groups (owner_id);
CREATE INDEX association_group_members_observation_id_idx
ON association_group_members (observation_id);
CREATE INDEX activity_projections_start_idx
ON activity_projections (start_ms DESC, observation_id);
CREATE INDEX route_plans_owner_id_idx ON route_plans (owner_id);
CREATE INDEX route_plan_revisions_plan_id_idx
ON route_plan_revisions (plan_id, created_at DESC);
