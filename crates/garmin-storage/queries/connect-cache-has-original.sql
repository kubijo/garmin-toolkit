SELECT EXISTS(
    SELECT 1 FROM connect_cache_originals
    WHERE
        user_id = ?
        AND source_id = ?
        AND activity_id = ?
        AND archive IS NOT NULL
) AS present;
