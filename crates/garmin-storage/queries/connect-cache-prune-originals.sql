DELETE
FROM connect_cache_originals
WHERE user_id = ? AND (source_id != ? OR activity_id NOT IN (
    SELECT activity_id
    FROM connect_cache_summaries
    WHERE user_id = ? AND source_id = ? AND generation = ?
));
