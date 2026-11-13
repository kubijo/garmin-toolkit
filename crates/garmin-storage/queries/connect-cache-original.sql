SELECT
    archive,
    failure
FROM connect_cache_originals
WHERE user_id = ? AND source_id = ? AND activity_id = ?;
