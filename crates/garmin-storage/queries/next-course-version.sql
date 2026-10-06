SELECT coalesce(max(version), 0) + 1 AS version
FROM (
    SELECT version FROM course_generations
    WHERE revision_id = ?1
    UNION ALL
    SELECT version FROM course_generation_deletions
    WHERE revision_id = ?1
) AS versions;
