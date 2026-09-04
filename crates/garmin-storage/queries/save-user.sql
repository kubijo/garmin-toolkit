INSERT INTO users (
    id,
    role,
    display_name,
    accent_rgba,
    avatar_artifact_id,
    unit_system,
    language,
    theme
)
SELECT
    ? AS id,
    ? AS role,
    ? AS display_name,
    ? AS accent_rgba,
    ? AS avatar_artifact_id,
    ? AS unit_system,
    ? AS language,
    ? AS theme
WHERE
    ? IS NULL OR EXISTS (
        SELECT 1
        FROM profile_avatars
        WHERE
            profile_avatars.owner_id = ?
            AND profile_avatars.original_artifact_id = ?
    )
ON CONFLICT (id) DO UPDATE SET
    role = excluded.role,
    display_name = excluded.display_name,
    accent_rgba = excluded.accent_rgba,
    avatar_artifact_id = excluded.avatar_artifact_id,
    unit_system = excluded.unit_system,
    language = excluded.language,
    theme = excluded.theme
RETURNING id;
