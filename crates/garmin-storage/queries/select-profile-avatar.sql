UPDATE users
SET avatar_artifact_id = ?
WHERE
    id = ?
    AND EXISTS (
        SELECT 1
        FROM profile_avatars
        WHERE
            profile_avatars.owner_id = users.id
            AND profile_avatars.original_artifact_id = ?
    )
RETURNING id;
