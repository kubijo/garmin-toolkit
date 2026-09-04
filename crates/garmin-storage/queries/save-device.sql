INSERT INTO devices (id, label)
VALUES (?, ?)
ON CONFLICT (id) DO UPDATE SET label = excluded.label;
