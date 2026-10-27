CREATE TABLE IF NOT EXISTS remote_approval_publications (
 approval_id TEXT PRIMARY KEY REFERENCES approval_audit(id), request TEXT NOT NULL CHECK(json_valid(request)),
 request_digest TEXT NOT NULL, created_at INTEGER NOT NULL CHECK(created_at>=0)
) STRICT;
CREATE TRIGGER IF NOT EXISTS retain_remote_approval_publication BEFORE DELETE ON remote_approval_publications BEGIN SELECT RAISE(ABORT,'permanent remote approval publication'); END;
CREATE TRIGGER IF NOT EXISTS immutable_remote_approval_publication BEFORE UPDATE ON remote_approval_publications BEGIN SELECT RAISE(ABORT,'immutable remote approval publication'); END;
CREATE TABLE IF NOT EXISTS remote_approval_receipts (
 approval_id TEXT PRIMARY KEY REFERENCES remote_approval_publications(approval_id),
 event_id TEXT NOT NULL, observed_at INTEGER NOT NULL CHECK(observed_at>=0)
) STRICT;
CREATE TRIGGER IF NOT EXISTS retain_remote_approval_receipt BEFORE DELETE ON remote_approval_receipts BEGIN SELECT RAISE(ABORT,'permanent remote approval receipt'); END;
CREATE TRIGGER IF NOT EXISTS immutable_remote_approval_receipt BEFORE UPDATE ON remote_approval_receipts BEGIN SELECT RAISE(ABORT,'immutable remote approval receipt'); END;
CREATE TABLE IF NOT EXISTS remote_approval_decisions (
 approval_id TEXT PRIMARY KEY REFERENCES remote_approval_receipts(approval_id), event_id TEXT NOT NULL UNIQUE,
 device_id TEXT NOT NULL, device_key TEXT NOT NULL, approved INTEGER NOT NULL CHECK(approved IN (0,1)),
 observed_at INTEGER NOT NULL CHECK(observed_at>=0)
) STRICT;
CREATE TRIGGER IF NOT EXISTS retain_remote_approval_decision BEFORE DELETE ON remote_approval_decisions BEGIN SELECT RAISE(ABORT,'permanent remote approval decision'); END;
CREATE TRIGGER IF NOT EXISTS immutable_remote_approval_decision BEFORE UPDATE ON remote_approval_decisions BEGIN SELECT RAISE(ABORT,'immutable remote approval decision'); END;
