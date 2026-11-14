SELECT
    source_id,
    generation,
    facts
FROM connect_cache_accounts
WHERE user_id = ? AND credential_hash = ?;
