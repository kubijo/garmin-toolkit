-- Keep serial allocation independent of deletions from generation history.
CREATE TABLE course_serial_counter (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    last_serial INTEGER NOT NULL CHECK (last_serial BETWEEN 0 AND 4294967295)
) STRICT;

INSERT INTO course_serial_counter (singleton, last_serial) VALUES (1, 0);

CREATE TABLE course_generations (
    id TEXT PRIMARY KEY NOT NULL,
    operation_id TEXT NOT NULL UNIQUE,
    owner_id TEXT NOT NULL,
    plan_id TEXT NOT NULL,
    revision_id TEXT NOT NULL,
    artifact_id TEXT NOT NULL UNIQUE REFERENCES artifacts (id),
    version INTEGER NOT NULL CHECK (version BETWEEN 1 AND 4294967295),
    serial INTEGER NOT NULL UNIQUE CHECK (serial BETWEEN 1 AND 4294967295),
    encoder_name TEXT NOT NULL CHECK (length(trim(encoder_name)) > 0),
    encoder_version TEXT NOT NULL,
    generated_at TEXT NOT NULL,
    UNIQUE (revision_id, version),
    FOREIGN KEY (plan_id, owner_id)
    REFERENCES route_plans (id, owner_id) ON DELETE CASCADE,
    FOREIGN KEY (revision_id, plan_id)
    REFERENCES route_plan_revisions (id, plan_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX course_generations_owner_idx ON course_generations (owner_id);
