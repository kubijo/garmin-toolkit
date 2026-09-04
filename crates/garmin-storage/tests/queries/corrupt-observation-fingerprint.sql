UPDATE observations
SET fingerprint_digest = zeroblob(32)
WHERE id = ?;
