SELECT
    EXISTS(
        SELECT 1 FROM garmin_connect_credentials
        WHERE user_id = ?
    ) AS configured;
