INSERT INTO users (
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
)
SELECT
    ? AS id,
    ? AS role,
    ? AS display_name,
    ? AS accent_rgba,
    ? AS avatar_artifact_id,
    ? AS unit_system,
    ? AS language,
    ? AS theme,
    ? AS show_hidden_files,
    ? AS inline_file_windows,
    ? AS running_target_km,
    ? AS cycling_target_km,
    ? AS swimming_target_km
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
    theme = excluded.theme,
    show_hidden_files = excluded.show_hidden_files,
    inline_file_windows = excluded.inline_file_windows,
    running_target_km = excluded.running_target_km,
    cycling_target_km = excluded.cycling_target_km,
    swimming_target_km = excluded.swimming_target_km
RETURNING id;
