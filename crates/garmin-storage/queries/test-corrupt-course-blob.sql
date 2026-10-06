UPDATE artifact_blobs SET bytes = zeroblob(byte_count)
WHERE digest = (
    SELECT digest FROM artifacts
    WHERE id = ?
);
