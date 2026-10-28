CREATE TABLE IF NOT EXISTS approval_audit (
 id TEXT PRIMARY KEY, proposal_digest TEXT NOT NULL, proposal TEXT NOT NULL,
 agent TEXT NOT NULL, dispatch TEXT NOT NULL, created_at INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS approval_audit_scope ON approval_audit(agent,created_at,id);
CREATE TRIGGER IF NOT EXISTS retain_approval_audit BEFORE DELETE ON approval_audit BEGIN SELECT RAISE(ABORT,'retain approval audit'); END;
CREATE TRIGGER IF NOT EXISTS immutable_approval_audit BEFORE UPDATE ON approval_audit BEGIN SELECT RAISE(ABORT,'immutable approval audit'); END;
CREATE TABLE IF NOT EXISTS approval_audit_events (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT, approval_id TEXT NOT NULL REFERENCES approval_audit(id),
 phase TEXT NOT NULL CHECK(phase IN ('offered','owner_allowed','owner_denied','policy_allowed','policy_denied','applying','submitted','completed','failed','unknown','expired')),
 observed_at INTEGER NOT NULL, detail TEXT NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS approval_audit_events_id ON approval_audit_events(approval_id,sequence);
CREATE TRIGGER IF NOT EXISTS retain_approval_event BEFORE DELETE ON approval_audit_events BEGIN SELECT RAISE(ABORT,'retain approval event'); END;
CREATE TRIGGER IF NOT EXISTS immutable_approval_event BEFORE UPDATE ON approval_audit_events BEGIN SELECT RAISE(ABORT,'immutable approval event'); END;
