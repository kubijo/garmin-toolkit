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
    inline_file_windows,
    running_target_km,
    cycling_target_km,
    swimming_target_km
FROM users
ORDER BY LOWER(display_name), id;
