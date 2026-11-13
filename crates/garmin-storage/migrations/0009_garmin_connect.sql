CREATE TABLE garmin_connect_credentials (
    user_id TEXT PRIMARY KEY NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    personal_api_key TEXT NOT NULL CHECK (
        length(personal_api_key) BETWEEN 1 AND 8192
    )
) STRICT;
