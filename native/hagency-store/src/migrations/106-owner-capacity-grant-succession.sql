-- Schema104: a pool's capacity is a chain of immutable grant receipts. A v3
-- successor (renewal: next UTC month; amendment: same month) is its own
-- owner_capacity_grants row; this table records how it joins the chain.
-- Statements are replay-safe for test fixtures that rewind user_version.
PRAGMA defer_foreign_keys=ON;
CREATE TABLE IF NOT EXISTS owner_capacity_grant_successions (
 grant_id TEXT PRIMARY KEY REFERENCES owner_capacity_grants(id),
 predecessor_grant_id TEXT NOT NULL REFERENCES owner_capacity_grants(id),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 kind TEXT NOT NULL CHECK(kind IN('renewal','amendment')),
 state TEXT NOT NULL CHECK(state IN('pending','effective','rejected','cancelled')),
 -- 0 while pending; the native flip time (amendment) or the successor's
 -- period start (renewal) once effective.
 effective_from_ms INTEGER NOT NULL CHECK(effective_from_ms>=0 AND effective_from_ms<=9007199254740991),
 command_id TEXT NOT NULL UNIQUE,
 created_at_ms INTEGER NOT NULL CHECK(created_at_ms>0 AND created_at_ms<=9007199254740991),
 decided_at_ms INTEGER CHECK(decided_at_ms IS NULL OR (decided_at_ms>0 AND decided_at_ms<=9007199254740991)),
 CHECK(grant_id<>predecessor_grant_id),
 CHECK((state='pending')=(decided_at_ms IS NULL)),
 CHECK(state<>'pending' OR effective_from_ms=0),
 CHECK(state<>'effective' OR effective_from_ms>0)
) STRICT;
CREATE INDEX IF NOT EXISTS owner_capacity_grant_successors ON owner_capacity_grant_successions(predecessor_grant_id,state);
-- One live successor per predecessor: a second one may follow only a
-- rejected successor or a cancelled renewal.
CREATE TRIGGER IF NOT EXISTS owner_capacity_grant_succession_single BEFORE INSERT ON owner_capacity_grant_successions
WHEN NEW.state<>'pending' OR NEW.effective_from_ms<>0 OR NEW.decided_at_ms IS NOT NULL
 OR EXISTS(SELECT 1 FROM owner_capacity_grant_successions s WHERE s.predecessor_grant_id=NEW.predecessor_grant_id AND s.state IN('pending','effective'))
 OR EXISTS(SELECT 1 FROM owner_capacity_grant_successions s WHERE s.predecessor_grant_id=NEW.grant_id)
BEGIN SELECT RAISE(ABORT,'capacity grant successor exists'); END;
CREATE TRIGGER IF NOT EXISTS owner_capacity_grant_succession_transition BEFORE UPDATE ON owner_capacity_grant_successions
WHEN NEW.grant_id IS NOT OLD.grant_id OR NEW.predecessor_grant_id IS NOT OLD.predecessor_grant_id
 OR NEW.profile IS NOT OLD.profile OR NEW.kind IS NOT OLD.kind OR NEW.command_id IS NOT OLD.command_id
 OR NEW.created_at_ms IS NOT OLD.created_at_ms
 OR NOT ((OLD.state='pending' AND NEW.state IN('effective','rejected'))
  OR (OLD.kind='renewal' AND OLD.state IN('pending','effective') AND NEW.state='cancelled'))
 OR (OLD.state='effective' AND NEW.effective_from_ms IS NOT OLD.effective_from_ms)
 OR (OLD.decided_at_ms IS NOT NULL AND NEW.decided_at_ms IS NOT OLD.decided_at_ms)
BEGIN SELECT RAISE(ABORT,'immutable capacity grant succession'); END;
CREATE TRIGGER IF NOT EXISTS owner_capacity_grant_succession_retained BEFORE DELETE ON owner_capacity_grant_successions
BEGIN SELECT RAISE(ABORT,'capacity grant succession history is retained'); END;

-- When a child was permanently fenced. A child's accounting grant is the
-- effective grant of its chain at min(now, fenced_at). Filled by a trigger so
-- no fence writer changes; children fenced before this schema count from 0.
CREATE TABLE IF NOT EXISTS owner_managed_fence_times (
 child_id TEXT PRIMARY KEY REFERENCES owner_managed_agents(id),
 fenced_at_ms INTEGER NOT NULL CHECK(fenced_at_ms>=0 AND fenced_at_ms<=9007199254740991)
) STRICT;
CREATE TRIGGER IF NOT EXISTS owner_managed_fence_times_immutable BEFORE UPDATE ON owner_managed_fence_times
BEGIN SELECT RAISE(ABORT,'immutable managed fence time'); END;
CREATE TRIGGER IF NOT EXISTS owner_managed_fence_times_retained BEFORE DELETE ON owner_managed_fence_times
BEGIN SELECT RAISE(ABORT,'managed fence time is retained'); END;
CREATE TRIGGER IF NOT EXISTS owner_managed_agent_fence_time AFTER UPDATE OF permanent_fence ON owner_managed_agents
WHEN NEW.permanent_fence=1 AND OLD.permanent_fence=0
BEGIN INSERT OR IGNORE INTO owner_managed_fence_times(child_id,fenced_at_ms) VALUES(NEW.id,CAST(unixepoch('subsec')*1000 AS INTEGER)); END;
INSERT OR IGNORE INTO owner_managed_fence_times(child_id,fenced_at_ms) SELECT id,0 FROM owner_managed_agents WHERE permanent_fence=1;
PRAGMA defer_foreign_keys=OFF;
