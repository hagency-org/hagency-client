-- A no-start receipt is distinct from actual-process stop evidence.
CREATE TABLE owner_shared_unstarted_attempts (
 command_id TEXT PRIMARY KEY REFERENCES owner_shared_successor_fences(command_id),
 payload TEXT NOT NULL, digest TEXT NOT NULL, ledger_revision INTEGER NOT NULL
) STRICT;
CREATE TRIGGER owner_shared_unstarted_attempts_immutable BEFORE UPDATE ON owner_shared_unstarted_attempts BEGIN SELECT RAISE(ABORT,'immutable unstarted evidence');END;
CREATE TRIGGER owner_shared_unstarted_attempts_retained BEFORE DELETE ON owner_shared_unstarted_attempts BEGIN SELECT RAISE(ABORT,'retained unstarted evidence');END;
CREATE TRIGGER owner_shared_unstarted_attempts_proof BEFORE INSERT ON owner_shared_unstarted_attempts WHEN NOT EXISTS(SELECT 1 FROM owner_shared_successor_fences f WHERE f.command_id=NEW.command_id AND f.claimed_revision IS NULL AND json_extract(f.command_payload,'$.version')=3 AND f.fence_revision<NEW.ledger_revision) OR EXISTS(SELECT 1 FROM owner_shared_successor_attempts a WHERE a.id=NEW.command_id) BEGIN SELECT RAISE(ABORT,'unclaimed permanent fence required');END;
CREATE TABLE owner_shared_budget_demand_terminals (
 demand_id TEXT PRIMARY KEY REFERENCES owner_shared_demands(id),
 payload TEXT NOT NULL, digest TEXT NOT NULL, ledger_revision INTEGER NOT NULL
) STRICT;
CREATE TRIGGER owner_shared_budget_demand_terminals_immutable BEFORE UPDATE ON owner_shared_budget_demand_terminals BEGIN SELECT RAISE(ABORT,'immutable demand terminal');END;
CREATE TRIGGER owner_shared_budget_demand_terminals_retained BEFORE DELETE ON owner_shared_budget_demand_terminals BEGIN SELECT RAISE(ABORT,'retained demand terminal');END;
