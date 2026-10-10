DELETE FROM artifacts
WHERE id = ? RETURNING digest;
