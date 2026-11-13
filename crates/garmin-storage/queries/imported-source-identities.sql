SELECT DISTINCT a.source_identity
FROM acquisitions AS a
INNER JOIN normalization_runs AS n ON a.id = n.acquisition_id
INNER JOIN observations AS o ON n.id = o.normalization_run_id
WHERE a.owner_id = ? AND a.source_id = ? AND o.kind = 'activity';
