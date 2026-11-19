SELECT
    a.source_identity,
    p.observation_id
FROM acquisitions AS a
INNER JOIN normalization_runs AS n ON a.id = n.acquisition_id
INNER JOIN activity_projections AS p ON n.id = p.normalization_run_id
WHERE a.owner_id = ? AND a.source_id = ?
ORDER BY a.source_identity, p.observation_id;
