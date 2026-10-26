CREATE TABLE IF NOT EXISTS execution_workspaces (
 dispatch TEXT PRIMARY KEY REFERENCES local_inbox(id), execution_id TEXT NOT NULL, workspace TEXT NOT NULL, requires_tree INTEGER NOT NULL CHECK(requires_tree IN (0,1)), captured_at INTEGER NOT NULL
) STRICT;
CREATE TRIGGER IF NOT EXISTS retain_execution_workspace BEFORE DELETE ON execution_workspaces BEGIN SELECT RAISE(ABORT,'retain execution workspace'); END;
CREATE TRIGGER IF NOT EXISTS immutable_execution_workspace BEFORE UPDATE ON execution_workspaces BEGIN SELECT RAISE(ABORT,'immutable execution workspace'); END;
CREATE TABLE IF NOT EXISTS recovery_reviews (
 id TEXT PRIMARY KEY, dispatch TEXT NOT NULL REFERENCES local_inbox(id), agent TEXT NOT NULL,
 digest TEXT NOT NULL, evidence TEXT NOT NULL, created_at INTEGER NOT NULL, expires INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS recovery_review_dispatch ON recovery_reviews(agent,dispatch);
CREATE TRIGGER IF NOT EXISTS retain_recovery_review BEFORE DELETE ON recovery_reviews BEGIN SELECT RAISE(ABORT,'retain recovery review'); END;
CREATE TRIGGER IF NOT EXISTS immutable_recovery_review BEFORE UPDATE ON recovery_reviews BEGIN SELECT RAISE(ABORT,'immutable recovery review'); END;
CREATE TABLE IF NOT EXISTS recovery_actions (
 id TEXT PRIMARY KEY, review_id TEXT NOT NULL UNIQUE REFERENCES recovery_reviews(id), agent TEXT NOT NULL,
 input TEXT NOT NULL, deciding_device TEXT NOT NULL, created_at INTEGER NOT NULL,
 phase TEXT NOT NULL CHECK(phase IN ('recorded','send_possible','sent','unknown')), event_id TEXT
) STRICT;
CREATE INDEX IF NOT EXISTS recovery_action_review ON recovery_actions(review_id,created_at,id);
CREATE TRIGGER IF NOT EXISTS retain_recovery_action BEFORE DELETE ON recovery_actions BEGIN SELECT RAISE(ABORT,'retain recovery action'); END;
CREATE TRIGGER IF NOT EXISTS immutable_recovery_action BEFORE UPDATE ON recovery_actions
 WHEN NEW.id!=OLD.id OR NEW.review_id!=OLD.review_id OR NEW.agent!=OLD.agent OR NEW.input!=OLD.input
 OR NEW.deciding_device!=OLD.deciding_device OR NEW.created_at!=OLD.created_at
 OR OLD.phase!='send_possible' OR NEW.phase NOT IN ('sent','unknown')
 OR OLD.event_id IS NOT NULL OR (NEW.phase='sent')!=(NEW.event_id IS NOT NULL)
 BEGIN SELECT RAISE(ABORT,'immutable recovery action or result'); END;
CREATE TABLE IF NOT EXISTS execution_inputs (dispatch TEXT PRIMARY KEY REFERENCES local_inbox(id), body TEXT NOT NULL) STRICT;
CREATE TRIGGER IF NOT EXISTS retain_execution_input BEFORE DELETE ON execution_inputs BEGIN SELECT RAISE(ABORT,'retain original stopped input'); END;
CREATE TRIGGER IF NOT EXISTS immutable_execution_input BEFORE UPDATE ON execution_inputs BEGIN SELECT RAISE(ABORT,'immutable original stopped input'); END;
CREATE TABLE IF NOT EXISTS execution_stops (dispatch TEXT PRIMARY KEY REFERENCES local_inbox(id), execution_id TEXT NOT NULL, scope TEXT NOT NULL CHECK(scope IN ('leader','whole_tree')), observed_at INTEGER NOT NULL) STRICT;
CREATE TRIGGER IF NOT EXISTS retain_execution_stop BEFORE DELETE ON execution_stops BEGIN SELECT RAISE(ABORT,'retain original execution stop'); END;
CREATE TRIGGER IF NOT EXISTS immutable_execution_stop BEFORE UPDATE ON execution_stops BEGIN SELECT RAISE(ABORT,'immutable original execution stop'); END;
