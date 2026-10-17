DELETE FROM artifact_blobs
WHERE digest = ? AND NOT EXISTS (
    SELECT 1 FROM artifacts
    WHERE artifacts.digest = artifact_blobs.digest
);
