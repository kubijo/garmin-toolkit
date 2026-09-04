INSERT INTO association_groups (
    id,
    owner_id,
    kind,
    basis,
    fingerprint_schema_name,
    fingerprint_schema_version,
    fingerprint_digest
)
VALUES (?, ?, ?, 'equal_fingerprint', ?, ?, ?)
ON CONFLICT (
    owner_id,
    kind,
    fingerprint_schema_name,
    fingerprint_schema_version,
    fingerprint_digest
) WHERE basis = 'equal_fingerprint'
DO UPDATE SET id = association_groups.id
RETURNING id;
