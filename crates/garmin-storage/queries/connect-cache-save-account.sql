INSERT INTO connect_cache_accounts (
    user_id, credential_hash, source_id, generation, facts
) VALUES (?, ?, ?, ?, ?)
ON CONFLICT (user_id) DO UPDATE SET
    credential_hash = excluded.credential_hash,
    source_id = excluded.source_id,
    generation = excluded.generation,
    facts = excluded.facts;
