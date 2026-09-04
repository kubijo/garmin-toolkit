INSERT INTO acquisitions (
    id,
    operation_id,
    artifact_id,
    owner_id,
    source_id,
    source_identity,
    acquired_at
)
VALUES (?, ?, ?, ?, ?, ?, ?)
ON CONFLICT DO UPDATE SET id = excluded.id
WHERE acquisitions.id = excluded.id
AND acquisitions.operation_id = excluded.operation_id
AND acquisitions.artifact_id = excluded.artifact_id
AND acquisitions.owner_id = excluded.owner_id
AND acquisitions.source_id = excluded.source_id
AND acquisitions.source_identity = excluded.source_identity
AND acquisitions.acquired_at = excluded.acquired_at
RETURNING id;
