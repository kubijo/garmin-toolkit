DELETE FROM connect_cache_summaries
WHERE
    user_id = ? AND source_id = ? AND generation = ?
    AND NOT EXISTS (
        SELECT 1 FROM connect_cache_accounts AS published
        WHERE
            published.user_id = connect_cache_summaries.user_id
            AND published.source_id = connect_cache_summaries.source_id
            AND published.generation = connect_cache_summaries.generation
    );
