-- Retain immutable prior allocation generations and every original fence.
PRAGMA defer_foreign_keys=ON;
CREATE TABLE owner_managed_agents_period_migration (
 id TEXT PRIMARY KEY,
 profile TEXT NOT NULL CHECK(length(profile)=64),
 agent_id TEXT NOT NULL,
 allocation_generation INTEGER NOT NULL DEFAULT 1 CHECK(allocation_generation>0 AND allocation_generation<=9007199254740991),
 request_id TEXT NOT NULL,
 project_reservation_id TEXT REFERENCES owner_project_reservations(id),
 pool_grant_id TEXT REFERENCES owner_capacity_grants(id),
 original_command_id TEXT NOT NULL UNIQUE,
 original_command_digest TEXT NOT NULL CHECK(length(original_command_digest)=64),
 original_command TEXT NOT NULL CHECK(json_valid(original_command)),
 definition_digest TEXT NOT NULL CHECK(length(definition_digest)=64),
 allocated_tokens INTEGER NOT NULL CHECK(allocated_tokens>0 AND allocated_tokens<=9007199254740991),
 budget_revision INTEGER NOT NULL CHECK(budget_revision>0 AND budget_revision<=9007199254740991),
 state TEXT NOT NULL CHECK(state IN('reserved','fenced')),
 permanent_fence INTEGER NOT NULL DEFAULT 0 CHECK(permanent_fence IN(0,1)),
 CHECK((state='fenced')=(permanent_fence=1)),
 CHECK((project_reservation_id IS NULL)<>(pool_grant_id IS NULL)),
 UNIQUE(profile,agent_id,allocation_generation),
 UNIQUE(profile,request_id)
) STRICT;
INSERT INTO owner_managed_agents_period_migration(id,profile,agent_id,allocation_generation,request_id,project_reservation_id,pool_grant_id,original_command_id,original_command_digest,original_command,definition_digest,allocated_tokens,budget_revision,state,permanent_fence) SELECT id,profile,agent_id,COALESCE(json_extract(original_command,'$.allocationGeneration'),1),request_id,project_reservation_id,pool_grant_id,original_command_id,original_command_digest,original_command,definition_digest,allocated_tokens,budget_revision,state,permanent_fence FROM owner_managed_agents;
DROP TABLE owner_managed_agents;
ALTER TABLE owner_managed_agents_period_migration RENAME TO owner_managed_agents;
CREATE INDEX owner_managed_project_children ON owner_managed_agents(project_reservation_id,id);
CREATE INDEX owner_managed_pool_children ON owner_managed_agents(pool_grant_id,id);
CREATE TRIGGER owner_managed_agent_binding_immutable BEFORE UPDATE ON owner_managed_agents
WHEN NEW.id IS NOT OLD.id OR NEW.profile IS NOT OLD.profile OR NEW.agent_id IS NOT OLD.agent_id
 OR NEW.allocation_generation IS NOT OLD.allocation_generation
 OR NEW.pool_grant_id IS NOT OLD.pool_grant_id OR NEW.request_id IS NOT OLD.request_id OR NEW.project_reservation_id IS NOT OLD.project_reservation_id
 OR NEW.original_command_id IS NOT OLD.original_command_id OR NEW.original_command_digest IS NOT OLD.original_command_digest
 OR NEW.original_command IS NOT OLD.original_command OR NEW.definition_digest IS NOT OLD.definition_digest
 OR NEW.allocated_tokens IS NOT OLD.allocated_tokens OR NEW.budget_revision IS NOT OLD.budget_revision
 OR NEW.permanent_fence<OLD.permanent_fence
BEGIN SELECT RAISE(ABORT,'immutable managed agent binding'); END;

CREATE TRIGGER owner_managed_agent_retained BEFORE DELETE ON owner_managed_agents
BEGIN SELECT RAISE(ABORT,'managed agent history is retained'); END;

CREATE TABLE owner_managed_fences_period_migration (
 profile TEXT NOT NULL CHECK(length(profile)=64),
 agent_id TEXT NOT NULL,
 allocation_generation INTEGER NOT NULL DEFAULT 1 CHECK(allocation_generation>0 AND allocation_generation<=9007199254740991),
 request_id TEXT NOT NULL,
 original_command_id TEXT NOT NULL UNIQUE,
 original_command_digest TEXT NOT NULL CHECK(length(original_command_digest)=64),
 original_command TEXT NOT NULL CHECK(json_valid(original_command)),
 project_reservation_id TEXT REFERENCES owner_project_reservations(id),
 pool_grant_id TEXT REFERENCES owner_capacity_grants(id),
 executor_device_id TEXT NOT NULL,
 fence_revision INTEGER NOT NULL CHECK(fence_revision>0 AND fence_revision<=9007199254740991),
 CHECK((project_reservation_id IS NULL)<>(pool_grant_id IS NULL)),
 PRIMARY KEY(profile,agent_id,allocation_generation),
 UNIQUE(profile,request_id)
) STRICT;
INSERT INTO owner_managed_fences_period_migration(profile,agent_id,allocation_generation,request_id,original_command_id,original_command_digest,original_command,project_reservation_id,pool_grant_id,executor_device_id,fence_revision) SELECT profile,agent_id,COALESCE(json_extract(original_command,'$.allocationGeneration'),1),request_id,original_command_id,original_command_digest,original_command,project_reservation_id,pool_grant_id,executor_device_id,fence_revision FROM owner_managed_fences;
DROP TABLE owner_managed_fences;
ALTER TABLE owner_managed_fences_period_migration RENAME TO owner_managed_fences;
CREATE TRIGGER owner_managed_fence_immutable BEFORE UPDATE ON owner_managed_fences
BEGIN SELECT RAISE(ABORT,'permanent managed agent fence'); END;
CREATE TRIGGER owner_managed_fence_retained BEFORE DELETE ON owner_managed_fences
BEGIN SELECT RAISE(ABORT,'managed fence history is retained'); END;

-- Validate actual referential integrity before clearing the deferred DROP-table
-- counters. The replacement retains every referenced row under the same name.
CREATE TEMP TABLE agent_period_migration_check(valid INTEGER NOT NULL CHECK(valid=1));
INSERT INTO agent_period_migration_check SELECT NOT EXISTS(SELECT 1 FROM pragma_foreign_key_check);
DROP TABLE agent_period_migration_check;
PRAGMA defer_foreign_keys=OFF;

CREATE TABLE IF NOT EXISTS owner_managed_periods (
 id TEXT PRIMARY KEY,
 child_id TEXT NOT NULL REFERENCES owner_managed_agents(id),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 agent_id TEXT NOT NULL,
 period_key TEXT NOT NULL,
 start_ms INTEGER NOT NULL CHECK(start_ms>0),
 end_ms INTEGER NOT NULL CHECK(end_ms>start_ms),
 grant_id TEXT NOT NULL REFERENCES owner_capacity_grants(id),
 tokens INTEGER NOT NULL CHECK(tokens>0 AND tokens<=9007199254740991),
 command_id TEXT NOT NULL UNIQUE,
 command_digest TEXT NOT NULL CHECK(length(command_digest)=64),
 ledger_revision INTEGER NOT NULL CHECK(ledger_revision>0),
 UNIQUE(child_id,period_key),
 UNIQUE(child_id,start_ms)
) STRICT;
CREATE INDEX IF NOT EXISTS owner_managed_periods_current ON owner_managed_periods(child_id,start_ms,end_ms);
CREATE TRIGGER IF NOT EXISTS owner_managed_period_immutable BEFORE UPDATE ON owner_managed_periods
BEGIN SELECT RAISE(ABORT,'immutable period authorization'); END;
CREATE TRIGGER IF NOT EXISTS owner_managed_period_retained BEFORE DELETE ON owner_managed_periods
BEGIN SELECT RAISE(ABORT,'period history retained'); END;
CREATE TRIGGER IF NOT EXISTS owner_managed_period_disjoint BEFORE INSERT ON owner_managed_periods
WHEN EXISTS(SELECT 1 FROM owner_managed_periods p
 WHERE p.profile=NEW.profile AND p.agent_id=NEW.agent_id
 AND p.start_ms<NEW.end_ms AND p.end_ms>NEW.start_ms)
BEGIN SELECT RAISE(ABORT,'agent period already authorized'); END;
CREATE TABLE IF NOT EXISTS owner_managed_call_periods (
 call_id TEXT PRIMARY KEY REFERENCES owner_managed_calls(id),
 period_id TEXT NOT NULL REFERENCES owner_managed_periods(id)
) STRICT;
CREATE INDEX IF NOT EXISTS owner_managed_calls_by_period ON owner_managed_call_periods(period_id,call_id);
CREATE TRIGGER IF NOT EXISTS owner_managed_call_period_immutable BEFORE UPDATE ON owner_managed_call_periods
BEGIN SELECT RAISE(ABORT,'immutable call period'); END;
CREATE TRIGGER IF NOT EXISTS owner_managed_call_period_retained BEFORE DELETE ON owner_managed_call_periods
BEGIN SELECT RAISE(ABORT,'call period history retained'); END;

-- Rebuilding the identity table must retain the monotonic fence-time hook.
CREATE TRIGGER owner_managed_agent_fence_time AFTER UPDATE OF permanent_fence ON owner_managed_agents
WHEN NEW.permanent_fence=1 AND OLD.permanent_fence=0
BEGIN INSERT OR IGNORE INTO owner_managed_fence_times(child_id,fenced_at_ms) VALUES(NEW.id,CAST(unixepoch('subsec')*1000 AS INTEGER)); END;
