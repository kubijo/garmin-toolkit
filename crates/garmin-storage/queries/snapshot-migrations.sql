SELECT
    version,
    success,
    checksum
FROM _sqlx_migrations
ORDER BY version;
