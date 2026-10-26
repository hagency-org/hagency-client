CREATE TABLE IF NOT EXISTS sent_files (
 id TEXT PRIMARY KEY, scope TEXT NOT NULL, dispatch TEXT NOT NULL REFERENCES local_inbox(id),
 execution TEXT NOT NULL, workspace TEXT NOT NULL, relative_path TEXT NOT NULL,
 manifest TEXT NOT NULL, content_hash TEXT NOT NULL, request_id TEXT NOT NULL UNIQUE,
 audit TEXT NOT NULL REFERENCES approval_audit(id), remote TEXT
);
CREATE TRIGGER IF NOT EXISTS retain_sent_file BEFORE DELETE ON sent_files BEGIN SELECT RAISE(ABORT,'permanent sent file'); END;
CREATE TRIGGER IF NOT EXISTS immutable_sent_file BEFORE UPDATE ON sent_files
 WHEN NEW.id<>OLD.id OR NEW.scope<>OLD.scope OR NEW.dispatch<>OLD.dispatch OR NEW.execution<>OLD.execution
 OR NEW.workspace<>OLD.workspace OR NEW.relative_path<>OLD.relative_path OR NEW.manifest<>OLD.manifest
 OR NEW.content_hash<>OLD.content_hash OR NEW.request_id<>OLD.request_id OR NEW.audit<>OLD.audit
 OR NEW.remote IS NULL
 BEGIN SELECT RAISE(ABORT,'immutable sent file'); END;
