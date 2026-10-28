CREATE TABLE IF NOT EXISTS received_files (
 id TEXT PRIMARY KEY, scope TEXT NOT NULL, dispatch TEXT NOT NULL REFERENCES local_inbox(id),
 execution TEXT NOT NULL, event TEXT NOT NULL, descriptor_digest TEXT NOT NULL,
 workspace TEXT NOT NULL, relative_path TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('reserved','write_possible','ready','failed')),
 receipt TEXT, CHECK ((state='ready')=(receipt IS NOT NULL))
);
CREATE TRIGGER IF NOT EXISTS retain_received_file BEFORE DELETE ON received_files BEGIN SELECT RAISE(ABORT,'permanent received file'); END;
CREATE TRIGGER IF NOT EXISTS immutable_received_file BEFORE UPDATE ON received_files
 WHEN NEW.id<>OLD.id OR NEW.scope<>OLD.scope OR NEW.dispatch<>OLD.dispatch OR NEW.execution<>OLD.execution
 OR NEW.event<>OLD.event OR NEW.descriptor_digest<>OLD.descriptor_digest OR NEW.workspace<>OLD.workspace OR NEW.relative_path<>OLD.relative_path
 OR NOT ((OLD.state='reserved' AND NEW.state IN ('write_possible','failed')) OR (OLD.state='write_possible' AND NEW.state='ready'))
 BEGIN SELECT RAISE(ABORT,'immutable received file'); END;
