INSERT INTO connect_cache_summaries (
    user_id, source_id, generation, activity_id, summary
) VALUES (?, ?, ?, ?, ?)
ON CONFLICT (user_id, source_id, generation, activity_id) DO UPDATE SET
    summary = excluded.summary;
