INSERT INTO connect_sync (user_id, state) VALUES (?, ?)
ON CONFLICT (user_id) DO UPDATE SET state = excluded.state;
