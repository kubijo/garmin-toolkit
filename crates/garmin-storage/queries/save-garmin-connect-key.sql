INSERT INTO garmin_connect_credentials (user_id, personal_api_key) VALUES (?, ?)
ON CONFLICT (user_id) DO UPDATE SET
    personal_api_key = excluded.personal_api_key;
