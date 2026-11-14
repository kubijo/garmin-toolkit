SELECT
    count(*) AS total,
    count(CASE WHEN o.archive IS NOT NULL THEN 1 END) AS downloaded,
    count(o.failure) AS unavailable
FROM connect_cache_summaries AS s LEFT JOIN connect_cache_originals AS o
    ON
        s.user_id = o.user_id
        AND s.source_id = o.source_id
        AND s.activity_id = o.activity_id
WHERE s.user_id = ? AND s.source_id = ? AND s.generation = ?;
