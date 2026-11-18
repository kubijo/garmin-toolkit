SELECT
    a.source_identity,
    MAX(p.ascent_mm) AS ascent_mm,
    MAX(p.average_heart_rate_bpm) AS average_heart_rate_bpm
FROM acquisitions AS a
INNER JOIN normalization_runs AS n ON a.id = n.acquisition_id
INNER JOIN activity_projections AS p ON n.id = p.normalization_run_id
WHERE a.owner_id = ? AND a.source_id = ?
GROUP BY a.source_identity
HAVING
    COUNT(*) = 1
    AND (COUNT(p.ascent_mm) = 1 OR COUNT(p.average_heart_rate_bpm) = 1);
