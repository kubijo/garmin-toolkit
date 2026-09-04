SELECT artifact_blobs.bytes
FROM artifacts
INNER JOIN artifact_blobs ON artifacts.digest = artifact_blobs.digest
WHERE artifacts.id = ?;
