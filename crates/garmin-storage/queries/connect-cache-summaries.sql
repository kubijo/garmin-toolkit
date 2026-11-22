SELECT
    s.summary,
    o.failure IS NOT NULL AS original_unavailable
FROM connect_cache_summaries AS s LEFT JOIN connect_cache_originals AS o
    ON
        s.user_id = o.user_id
        AND s.source_id = o.source_id
        AND s.activity_id = o.activity_id
WHERE s.user_id = ? AND s.source_id = ? AND s.generation = ?
ORDER BY s.activity_id DESC;
