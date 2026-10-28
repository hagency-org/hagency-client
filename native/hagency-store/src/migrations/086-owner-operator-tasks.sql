-- Canonical operator Tasks are separate from runner tasks and shared work.
-- No source is read or imported by this schema migration.
CREATE TABLE owner_operator_task_histories (
    profile TEXT PRIMARY KEY REFERENCES owner_resource_adoptions(profile),
    migration_id TEXT NOT NULL UNIQUE REFERENCES owner_resource_adoptions(id),
    receipt_digest TEXT NOT NULL UNIQUE CHECK(length(receipt_digest)=64),
    receipt TEXT NOT NULL CHECK(json_valid(receipt) AND length(CAST(receipt AS BLOB))<=8192)
) STRICT;
CREATE TRIGGER owner_operator_task_history_immutable BEFORE UPDATE ON owner_operator_task_histories
BEGIN SELECT RAISE(ABORT,'retain original operator task history provenance'); END;
CREATE TRIGGER owner_operator_task_history_retained BEFORE DELETE ON owner_operator_task_histories
BEGIN SELECT RAISE(ABORT,'retain original operator task history provenance'); END;
CREATE TABLE owner_operator_tasks (
    profile TEXT NOT NULL REFERENCES owner_operator_task_histories(profile),
    id TEXT NOT NULL CHECK(length(id) BETWEEN 1 AND 64),
    revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
    title TEXT NOT NULL CHECK(length(title) BETWEEN 1 AND 255),
    description TEXT NOT NULL CHECK(length(description)<=4096),
    status TEXT NOT NULL CHECK(status IN ('created','accepted','in_progress','blocked','done')),
    priority TEXT NOT NULL CHECK(priority IN ('p0','p1','p2','p3')),
    granularity TEXT NOT NULL CHECK(granularity IN ('epic','task','subtask')),
    assignee TEXT CHECK(assignee IS NULL OR length(assignee)<=128),
    created_by TEXT CHECK(created_by IS NULL OR length(created_by)<=128),
    created_at INTEGER NOT NULL CHECK(created_at BETWEEN 0 AND 9007199254740991),
    updated_at INTEGER NOT NULL CHECK(updated_at BETWEEN 0 AND 9007199254740991),
    started_at INTEGER CHECK(started_at IS NULL OR started_at BETWEEN 0 AND 9007199254740991),
    completed_at INTEGER CHECK(completed_at IS NULL OR completed_at BETWEEN 0 AND 9007199254740991),
    heartbeat_at INTEGER CHECK(heartbeat_at IS NULL OR heartbeat_at BETWEEN 0 AND 9007199254740991),
    waiting_reason TEXT CHECK(waiting_reason IS NULL OR length(waiting_reason)<=1024),
    waiting_until TEXT CHECK(waiting_until IS NULL OR length(waiting_until)<=64),
    parent_id TEXT CHECK(parent_id IS NULL OR length(parent_id)<=64),
    labels TEXT NOT NULL CHECK(json_valid(labels) AND json_type(labels)='array' AND json_array_length(labels)<=20 AND length(CAST(labels AS BLOB))<=8192),
    PRIMARY KEY(profile,id)
) STRICT;
CREATE INDEX owner_operator_task_order ON owner_operator_tasks(profile,created_at DESC,id DESC);
CREATE TABLE owner_operator_task_comments (
    profile TEXT NOT NULL,
    task_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 99),
    author TEXT NOT NULL CHECK(length(author)<=128),
    text TEXT NOT NULL CHECK(length(text)<=4096),
    at_ms INTEGER NOT NULL CHECK(at_ms BETWEEN 0 AND 9007199254740991),
    PRIMARY KEY(profile,task_id,ordinal),
    FOREIGN KEY(profile,task_id) REFERENCES owner_operator_tasks(profile,id) ON DELETE CASCADE
) STRICT;
