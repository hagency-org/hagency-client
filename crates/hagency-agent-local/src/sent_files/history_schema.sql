CREATE TABLE IF NOT EXISTS sent_file_absence (
 id TEXT PRIMARY KEY REFERENCES sent_files(id), proof TEXT NOT NULL CHECK(json_valid(proof)),
 observed_at INTEGER NOT NULL CHECK(observed_at>=0)
);
CREATE TRIGGER IF NOT EXISTS valid_sent_file_absence BEFORE INSERT ON sent_file_absence
 WHEN NOT EXISTS(SELECT 1 FROM sent_files s WHERE s.id=NEW.id AND s.remote IS NULL
 AND json_extract(NEW.proof,'$.request.dispatchId')=s.dispatch
 AND json_extract(NEW.proof,'$.request.executionId')=s.execution
 AND json_extract(NEW.proof,'$.request.requestId')=s.request_id
 AND json_type(NEW.proof,'$.delivery')='null'
 AND json_type(NEW.proof,'$.preparationClosed')='true')
 BEGIN SELECT RAISE(ABORT,'invalid file absence'); END;
CREATE TRIGGER IF NOT EXISTS retain_sent_file_absence BEFORE DELETE ON sent_file_absence BEGIN SELECT RAISE(ABORT,'permanent file absence'); END;
CREATE TRIGGER IF NOT EXISTS immutable_sent_file_absence BEFORE UPDATE ON sent_file_absence BEGIN SELECT RAISE(ABORT,'immutable file absence'); END;
CREATE TRIGGER IF NOT EXISTS reject_absent_file_update BEFORE UPDATE ON sent_files
 WHEN EXISTS(SELECT 1 FROM sent_file_absence a WHERE a.id=OLD.id)
 BEGIN SELECT RAISE(ABORT,'final file absence'); END;
