-- Legacy routes remain readable; their candidate locators cannot be recovered.
CREATE TABLE route_import_receipts (
    acquisition_id TEXT PRIMARY KEY NOT NULL,
    artifact_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    plan_id TEXT NOT NULL UNIQUE,
    revision_id TEXT NOT NULL UNIQUE,
    candidate_kind TEXT NOT NULL
    CHECK (candidate_kind IN ('track_segment', 'route')),
    candidate_index INTEGER NOT NULL CHECK (candidate_index >= 0),
    segment_index INTEGER CHECK (segment_index >= 0),
    parser_name TEXT NOT NULL CHECK (length(trim(parser_name)) > 0),
    parser_version TEXT NOT NULL,
    CHECK (
        (candidate_kind = 'track_segment' AND segment_index IS NOT NULL)
        OR (candidate_kind = 'route' AND segment_index IS NULL)
    ),
    FOREIGN KEY (acquisition_id, artifact_id, owner_id)
    REFERENCES acquisitions (id, artifact_id, owner_id) ON DELETE CASCADE,
    FOREIGN KEY (plan_id, owner_id)
    REFERENCES route_plans (id, owner_id) ON DELETE CASCADE,
    FOREIGN KEY (revision_id, plan_id)
    REFERENCES route_plan_revisions (id, plan_id) ON DELETE CASCADE
) STRICT;
