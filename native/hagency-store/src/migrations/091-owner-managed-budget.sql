-- Schema89: append-only child budget evidence. Original reserve/call rows stay immutable.
CREATE TABLE owner_managed_budget_commands (
 command_id TEXT PRIMARY KEY,
 child_id TEXT NOT NULL REFERENCES owner_managed_agents(id),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 executor_device_id TEXT NOT NULL,
 top_up_id TEXT NOT NULL UNIQUE,
 command_digest TEXT NOT NULL CHECK(length(command_digest)=64),
 command_payload TEXT NOT NULL CHECK(json_valid(command_payload) AND length(CAST(command_payload AS BLOB))<=16384),
 receipt_payload TEXT NOT NULL CHECK(json_valid(receipt_payload) AND length(CAST(receipt_payload AS BLOB))<=8192),
 receipt_digest TEXT NOT NULL CHECK(length(receipt_digest)=64),
 ledger_revision INTEGER NOT NULL UNIQUE CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991)
) STRICT;
CREATE TABLE owner_managed_budget_deltas (
 command_id TEXT PRIMARY KEY REFERENCES owner_managed_budget_commands(command_id),
 child_id TEXT NOT NULL REFERENCES owner_managed_agents(id),
 budget_revision INTEGER NOT NULL CHECK(budget_revision>1 AND budget_revision<=9007199254740991),
 previous_digest TEXT NOT NULL CHECK(length(previous_digest)=64),
 budget_digest TEXT NOT NULL CHECK(length(budget_digest)=64),
 applied_tokens INTEGER NOT NULL CHECK(applied_tokens>0 AND applied_tokens<=9007199254740991),
 effective_ceiling_tokens INTEGER NOT NULL CHECK(effective_ceiling_tokens>0 AND effective_ceiling_tokens<=9007199254740991),
 ledger_revision INTEGER NOT NULL UNIQUE CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991),
 UNIQUE(child_id,budget_revision),
 UNIQUE(child_id,previous_digest)
) STRICT;
CREATE TRIGGER owner_managed_budget_delta_exact BEFORE INSERT ON owner_managed_budget_deltas WHEN NOT EXISTS(
 SELECT 1 FROM owner_managed_budget_commands c WHERE c.command_id=NEW.command_id AND c.child_id=NEW.child_id
 AND c.ledger_revision=NEW.ledger_revision AND json_extract(c.receipt_payload,'$.state')='applied'
 AND json_extract(c.receipt_payload,'$.budgetRevision')=NEW.budget_revision
 AND json_extract(c.receipt_payload,'$.previousBudgetDigest')=NEW.previous_digest
 AND json_extract(c.receipt_payload,'$.budgetDigest')=NEW.budget_digest
 AND json_extract(c.receipt_payload,'$.appliedDeltaTokens')=NEW.applied_tokens
 AND json_extract(c.receipt_payload,'$.effectiveCeilingTokens')=NEW.effective_ceiling_tokens
) BEGIN SELECT RAISE(ABORT,'exact applied budget receipt required'); END;
CREATE TRIGGER owner_managed_budget_commands_immutable BEFORE UPDATE ON owner_managed_budget_commands BEGIN SELECT RAISE(ABORT,'immutable budget disposition'); END;
CREATE TRIGGER owner_managed_budget_commands_retained BEFORE DELETE ON owner_managed_budget_commands BEGIN SELECT RAISE(ABORT,'retained budget disposition'); END;
CREATE TRIGGER owner_managed_budget_deltas_immutable BEFORE UPDATE ON owner_managed_budget_deltas BEGIN SELECT RAISE(ABORT,'immutable applied budget'); END;
CREATE TRIGGER owner_managed_budget_deltas_retained BEFORE DELETE ON owner_managed_budget_deltas BEGIN SELECT RAISE(ABORT,'retained applied budget'); END;
