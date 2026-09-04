INSERT INTO artifacts (id, digest, media_type)
VALUES (?, ?, ?)
ON CONFLICT DO UPDATE SET id = excluded.id
WHERE artifacts.digest = excluded.digest
AND artifacts.media_type = excluded.media_type
RETURNING id;
