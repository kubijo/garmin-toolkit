SELECT id
FROM observations
WHERE owner_id = ? AND kind = 'activity'
ORDER BY id;
