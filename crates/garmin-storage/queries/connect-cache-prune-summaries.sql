DELETE
FROM connect_cache_summaries
WHERE user_id = ? AND (source_id != ? OR generation != ?);
