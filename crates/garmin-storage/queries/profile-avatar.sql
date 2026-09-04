SELECT
    profile_avatars.original_artifact_id,
    profile_avatars.thumbnail_artifact_id,
    profile_avatars.original_width,
    profile_avatars.original_height,
    artifact_blobs.bytes AS thumbnail_bytes
FROM users
INNER JOIN profile_avatars
    ON
        users.id = profile_avatars.owner_id
        AND users.avatar_artifact_id = profile_avatars.original_artifact_id
INNER JOIN artifacts
    ON profile_avatars.thumbnail_artifact_id = artifacts.id
INNER JOIN artifact_blobs
    ON artifacts.digest = artifact_blobs.digest
WHERE users.id = ?;
