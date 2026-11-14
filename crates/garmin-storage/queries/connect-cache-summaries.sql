SELECT summary
FROM connect_cache_summaries
WHERE user_id = ? AND source_id = ? AND generation = ?
ORDER BY activity_id DESC;
