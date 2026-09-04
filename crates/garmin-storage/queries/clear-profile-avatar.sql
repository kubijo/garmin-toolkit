UPDATE users
SET avatar_artifact_id = NULL
WHERE id = ?
RETURNING id;
