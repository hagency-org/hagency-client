CREATE TABLE IF NOT EXISTS remote_verdict_publications (
 approval_id TEXT PRIMARY KEY, request_digest TEXT NOT NULL, record TEXT NOT NULL CHECK(json_valid(record))
) STRICT;
CREATE TRIGGER IF NOT EXISTS retain_remote_verdict_publication BEFORE DELETE ON remote_verdict_publications BEGIN SELECT RAISE(ABORT,'permanent remote verdict publication'); END;
CREATE TRIGGER IF NOT EXISTS immutable_remote_verdict_publication BEFORE UPDATE ON remote_verdict_publications BEGIN SELECT RAISE(ABORT,'immutable remote verdict publication'); END;
CREATE TABLE IF NOT EXISTS remote_verdict_receipts (
 approval_id TEXT PRIMARY KEY REFERENCES remote_verdict_publications(approval_id),
 event_id TEXT NOT NULL UNIQUE, observed_at INTEGER NOT NULL CHECK(observed_at>=0)
) STRICT;
CREATE TRIGGER IF NOT EXISTS retain_remote_verdict_receipt BEFORE DELETE ON remote_verdict_receipts BEGIN SELECT RAISE(ABORT,'permanent remote verdict receipt'); END;
CREATE TRIGGER IF NOT EXISTS immutable_remote_verdict_receipt BEFORE UPDATE ON remote_verdict_receipts BEGIN SELECT RAISE(ABORT,'immutable remote verdict receipt'); END;
