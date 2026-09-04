INSERT INTO sources (id, owner_id, label, device_id)
VALUES (?, ?, ?, ?)
ON CONFLICT (id) DO UPDATE SET
    label = excluded.label,
    device_id = excluded.device_id
WHERE sources.owner_id = excluded.owner_id
RETURNING id;
