SELECT
    id,
    role,
    display_name,
    accent_rgba,
    avatar_artifact_id,
    unit_system,
    language,
    theme,
    show_hidden_files,
    inline_file_windows
FROM users
WHERE id = ?;
