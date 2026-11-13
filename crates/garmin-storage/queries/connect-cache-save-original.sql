INSERT INTO connect_cache_originals (
    user_id, source_id, activity_id, archive, failure
) VALUES (?, ?, ?, ?, ?)
ON CONFLICT (user_id, source_id, activity_id) DO UPDATE SET
    archive = excluded.archive, failure = excluded.failure;
