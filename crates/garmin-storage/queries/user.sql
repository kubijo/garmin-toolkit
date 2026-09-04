SELECT
    id,
    role,
    display_name,
    accent_rgba,
    avatar_artifact_id,
    unit_system,
    language,
    theme
FROM users
WHERE id = ?;
