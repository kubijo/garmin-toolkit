-- Apalis SQLite 1.1.0 schema, through upstream migration 20261020120000.
-- Derived from apalis-sqlite, copyright (c) 2025 apalis, under the MIT license.
-- Full license text is included in the dependency license bundles.
-- Included in the application migrator for one snapshot schema history.
-- Schema parity with the pinned upstream migrations is tested in jobs/tests.rs.
-- Do not call SqliteStorage::setup: its PRAGMAs disable synchronous writes.
CREATE TABLE workers (
    id TEXT NOT NULL UNIQUE,
    worker_type TEXT NOT NULL,
    storage_name TEXT NOT NULL,
    layers TEXT,
    last_seen INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    started_at INTEGER
);

CREATE TABLE jobs (
    job BLOB NOT NULL,
    id TEXT NOT NULL UNIQUE,
    job_type TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'Pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 25,
    run_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    last_result TEXT,
    lock_at INTEGER,
    lock_by TEXT,
    done_at INTEGER,
    priority INTEGER NOT NULL DEFAULT 0,
    metadata TEXT,
    idempotency_key TEXT,
    PRIMARY KEY (id),
    FOREIGN KEY (lock_by) REFERENCES workers (id)
);

CREATE INDEX idx ON workers (id);
CREATE INDEX lidx ON jobs (lock_by);
CREATE INDEX lsidx ON workers (last_seen);
CREATE INDEX wtidx ON workers (worker_type);
CREATE INDEX idx_jobs_claimable ON jobs (status, attempts)
WHERE status IN ('Queued', 'Pending', 'Failed');
CREATE INDEX idx_jobs_completed_done_at ON jobs (status, done_at, run_at)
WHERE status IN ('Done', 'Failed', 'Killed') AND done_at IS NOT NULL;
CREATE INDEX idx_jobs_failed ON jobs (
    job_type, priority DESC, run_at ASC, id ASC
)
WHERE status = 'Failed' AND attempts < max_attempts;
CREATE UNIQUE INDEX idx_jobs_idempotency_key ON jobs (
    job_type, idempotency_key
);
CREATE INDEX idx_jobs_job_type_status_run_at ON jobs (job_type, status, run_at);
CREATE INDEX idx_jobs_pending ON jobs (
    job_type, priority DESC, run_at ASC, id ASC
)
WHERE status = 'Pending' AND lock_by IS NULL;
CREATE INDEX idx_jobs_running ON jobs (run_at)
WHERE status = 'Running';
CREATE INDEX idx_jobs_status_recent ON jobs (status, done_at DESC, run_at DESC);
CREATE INDEX idx_jobs_status_run_at ON jobs (status, run_at);
CREATE INDEX idx_jobs_status_type_recent ON jobs (
    status, job_type, done_at DESC, run_at DESC
);
