INSERT INTO artifact_blobs (digest, byte_count, bytes)
VALUES (?, ?, ?)
ON CONFLICT DO UPDATE SET digest = excluded.digest
WHERE artifact_blobs.byte_count = excluded.byte_count
AND artifact_blobs.bytes = excluded.bytes
RETURNING digest;
