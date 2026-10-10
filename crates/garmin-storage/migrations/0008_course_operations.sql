-- Several generation requests can resolve to the same immutable artifact.
-- Keep these receipts after deletion so a retry cannot recreate deleted bytes.
CREATE TABLE course_generation_operations (
    operation_id TEXT PRIMARY KEY NOT NULL,
    generation_id TEXT NOT NULL,
    owner_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE
) STRICT;

INSERT INTO course_generation_operations (operation_id, generation_id, owner_id)
SELECT
    operation_id,
    id,
    owner_id
FROM course_generations
UNION ALL
SELECT
    operation_id,
    id,
    owner_id
FROM course_generation_deletions;

CREATE INDEX course_generation_reuse_idx
ON course_generations (
    revision_id, encoder_name, encoder_version, version DESC
);
