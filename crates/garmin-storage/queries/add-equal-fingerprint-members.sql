INSERT INTO association_group_members (
    association_group_id,
    observation_id,
    owner_id,
    kind
)
SELECT
    ? AS association_group_id,
    id,
    owner_id,
    kind
FROM observations
WHERE
    owner_id = ?
    AND kind = ?
    AND fingerprint_schema_name = ?
    AND fingerprint_schema_version = ?
    AND fingerprint_digest = ?
ON CONFLICT DO NOTHING;
