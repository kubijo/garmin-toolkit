CREATE TABLE connect_cache_accounts (
    user_id TEXT PRIMARY KEY NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    credential_hash TEXT NOT NULL,
    source_id TEXT NOT NULL,
    generation TEXT NOT NULL,
    facts TEXT NOT NULL
) STRICT;

CREATE TABLE connect_cache_summaries (
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    source_id TEXT NOT NULL,
    generation TEXT NOT NULL,
    activity_id INTEGER NOT NULL CHECK (activity_id > 0),
    summary TEXT NOT NULL,
    PRIMARY KEY (user_id, source_id, generation, activity_id)
) STRICT;

CREATE TABLE connect_cache_originals (
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    source_id TEXT NOT NULL,
    activity_id INTEGER NOT NULL CHECK (activity_id > 0),
    archive BLOB,
    failure TEXT,
    CHECK ((archive IS NOT NULL) != (failure IS NOT NULL)),
    PRIMARY KEY (user_id, source_id, activity_id)
) STRICT;
