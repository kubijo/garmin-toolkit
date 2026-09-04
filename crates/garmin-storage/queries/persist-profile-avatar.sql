INSERT INTO profile_avatars (
    owner_id,
    acquisition_id,
    original_artifact_id,
    thumbnail_artifact_id,
    original_width,
    original_height
)
VALUES (?, ?, ?, ?, ?, ?)
ON CONFLICT DO UPDATE SET owner_id = excluded.owner_id
WHERE profile_avatars.acquisition_id = excluded.acquisition_id
AND profile_avatars.original_artifact_id = excluded.original_artifact_id
AND profile_avatars.thumbnail_artifact_id = excluded.thumbnail_artifact_id
AND profile_avatars.original_width = excluded.original_width
AND profile_avatars.original_height = excluded.original_height
RETURNING owner_id;
