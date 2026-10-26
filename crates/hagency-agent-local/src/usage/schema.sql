CREATE TABLE IF NOT EXISTS call_observations (
 call_id TEXT PRIMARY KEY REFERENCES calls(id), started_at INTEGER NOT NULL CHECK(started_at>=0)
) STRICT;
CREATE INDEX IF NOT EXISTS call_observations_time ON call_observations(started_at,call_id);
CREATE TRIGGER IF NOT EXISTS permanent_call_time BEFORE UPDATE ON call_observations BEGIN SELECT RAISE(ABORT,'immutable call time'); END;
CREATE TRIGGER IF NOT EXISTS retain_call_time BEFORE DELETE ON call_observations BEGIN SELECT RAISE(ABORT,'retain call time'); END;
CREATE TABLE IF NOT EXISTS usage_alerts (
 id TEXT PRIMARY KEY, scope TEXT NOT NULL, window TEXT NOT NULL, revision INTEGER NOT NULL,
 threshold INTEGER NOT NULL CHECK(threshold IN (80,100)), token_limit INTEGER NOT NULL,
 spent INTEGER NOT NULL, held INTEGER NOT NULL, observed_at INTEGER NOT NULL,
 acknowledged_at INTEGER
) STRICT;
CREATE INDEX IF NOT EXISTS usage_alerts_scope ON usage_alerts(scope,observed_at,id);
CREATE TRIGGER IF NOT EXISTS retain_usage_alert BEFORE DELETE ON usage_alerts BEGIN SELECT RAISE(ABORT,'retain usage alert'); END;
CREATE TRIGGER IF NOT EXISTS immutable_usage_alert BEFORE UPDATE ON usage_alerts
 WHEN NEW.id IS NOT OLD.id OR NEW.scope IS NOT OLD.scope OR NEW.window IS NOT OLD.window
 OR NEW.revision IS NOT OLD.revision OR NEW.threshold IS NOT OLD.threshold
 OR NEW.token_limit IS NOT OLD.token_limit OR NEW.spent IS NOT OLD.spent
 OR NEW.held IS NOT OLD.held OR NEW.observed_at IS NOT OLD.observed_at
 OR (OLD.acknowledged_at IS NOT NULL AND NEW.acknowledged_at IS NOT OLD.acknowledged_at)
 BEGIN SELECT RAISE(ABORT,'immutable usage alert'); END;
