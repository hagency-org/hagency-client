-- V2 task transfer payloads are immutable and committed with the original plan
-- before any source fence. This migration never reads/imports a source.
CREATE TABLE owner_operator_task_transfer_tasks (
 migration_id TEXT NOT NULL REFERENCES owner_resource_adoptions(id),
 task_id TEXT NOT NULL CHECK(length(task_id) BETWEEN 1 AND 64),
 payload_digest TEXT NOT NULL CHECK(length(payload_digest)=64),
 payload TEXT NOT NULL CHECK(json_valid(payload) AND length(CAST(payload AS BLOB))<=65536),
 PRIMARY KEY(migration_id,task_id)
) STRICT;
CREATE TABLE owner_operator_task_transfer_comments (
 migration_id TEXT NOT NULL REFERENCES owner_resource_adoptions(id),
 sequence INTEGER NOT NULL CHECK(sequence BETWEEN 1 AND 9007199254740991),
 task_id TEXT NOT NULL,
 ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 99),
 payload_digest TEXT NOT NULL CHECK(length(payload_digest)=64),
 payload TEXT NOT NULL CHECK(json_valid(payload) AND length(CAST(payload AS BLOB))<=65536),
 PRIMARY KEY(migration_id,sequence),
 UNIQUE(migration_id,task_id,ordinal),
 FOREIGN KEY(migration_id,task_id) REFERENCES owner_operator_task_transfer_tasks(migration_id,task_id)
) STRICT;
CREATE TRIGGER owner_operator_task_transfer_task_immutable BEFORE UPDATE ON owner_operator_task_transfer_tasks
BEGIN SELECT RAISE(ABORT,'retain original operator task transfer'); END;
CREATE TRIGGER owner_operator_task_transfer_task_retained BEFORE DELETE ON owner_operator_task_transfer_tasks
BEGIN SELECT RAISE(ABORT,'retain original operator task transfer'); END;
CREATE TRIGGER owner_operator_task_transfer_comment_immutable BEFORE UPDATE ON owner_operator_task_transfer_comments
BEGIN SELECT RAISE(ABORT,'retain original operator task transfer'); END;
CREATE TRIGGER owner_operator_task_transfer_comment_retained BEFORE DELETE ON owner_operator_task_transfer_comments
BEGIN SELECT RAISE(ABORT,'retain original operator task transfer'); END;
