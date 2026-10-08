CREATE TABLE batches (
    id TEXT PRIMARY KEY NOT NULL,
    created_at INTEGER NOT NULL,
    finished_at INTEGER
);
CREATE TABLE jobs (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT UNIQUE NOT NULL,
    batch_id TEXT NOT NULL REFERENCES batches(id),
    ordinal INTEGER NOT NULL,
    source_key TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('pending','processing','completed','completed_with_warnings','rejected','failed')),
    report TEXT NOT NULL CHECK(json_valid(report)),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts BETWEEN 0 AND 3),
    recover INTEGER NOT NULL DEFAULT 0,
    available_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    started_at INTEGER,
    updated_at INTEGER NOT NULL,
    finished_at INTEGER,
    UNIQUE(batch_id, ordinal)
);
CREATE INDEX jobs_ready ON jobs(status, available_at, sequence);
CREATE INDEX jobs_batch ON jobs(batch_id, ordinal);
CREATE TABLE attempts (
    job_id TEXT NOT NULL REFERENCES jobs(id),
    number INTEGER NOT NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    outcome TEXT NOT NULL,
    report TEXT NOT NULL CHECK(json_valid(report)),
    PRIMARY KEY(job_id, number)
);
