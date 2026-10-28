-- Schema90: v3 startup and no-start evidence. Earlier protocol bytes are retained.
CREATE TABLE owner_shared_budget_start_intents (
 id TEXT PRIMARY KEY,
 child_id TEXT NOT NULL REFERENCES owner_shared_runtime_lifetimes(child_id),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 device_id TEXT NOT NULL,
 device_generation INTEGER NOT NULL CHECK(device_generation>0 AND device_generation<=9007199254740991),
 predecessor_digest TEXT NOT NULL CHECK(length(predecessor_digest)=64),
 original_runtime_digest TEXT NOT NULL CHECK(length(original_runtime_digest)=64),
 original_runtime_payload TEXT NOT NULL CHECK(json_valid(original_runtime_payload) AND length(CAST(original_runtime_payload AS BLOB))<=16384),
 input_payload TEXT NOT NULL CHECK(json_valid(input_payload) AND length(CAST(input_payload AS BLOB))<=16384),
 input_digest TEXT NOT NULL CHECK(length(input_digest)=64),
 ledger_revision INTEGER NOT NULL UNIQUE CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991),
 UNIQUE(child_id,predecessor_digest)
) STRICT;
CREATE TABLE owner_shared_budget_work_refusals (
 work_id TEXT PRIMARY KEY REFERENCES owner_shared_work_inbox(id),
 call_id TEXT NOT NULL UNIQUE REFERENCES owner_managed_calls(id),
 command_digest TEXT NOT NULL CHECK(length(command_digest)=64),
 refusal_payload TEXT NOT NULL CHECK(json_valid(refusal_payload) AND length(CAST(refusal_payload AS BLOB))<=16384),
 refusal_digest TEXT NOT NULL CHECK(length(refusal_digest)=64),
 observation_id TEXT NOT NULL UNIQUE REFERENCES owner_shared_work_observations(id),
 ledger_revision INTEGER NOT NULL UNIQUE CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991)
) STRICT;
CREATE TRIGGER owner_shared_budget_start_intents_immutable BEFORE UPDATE ON owner_shared_budget_start_intents BEGIN SELECT RAISE(ABORT,'immutable budget start intent'); END;
CREATE TRIGGER owner_shared_budget_start_intents_retained BEFORE DELETE ON owner_shared_budget_start_intents BEGIN SELECT RAISE(ABORT,'retained budget start intent'); END;
CREATE TRIGGER owner_shared_budget_work_refusals_immutable BEFORE UPDATE ON owner_shared_budget_work_refusals BEGIN SELECT RAISE(ABORT,'immutable budget refusal'); END;
CREATE TRIGGER owner_shared_budget_work_refusals_retained BEFORE DELETE ON owner_shared_budget_work_refusals BEGIN SELECT RAISE(ABORT,'retained budget refusal'); END;
-- Each applied budget head gets an immutable assessment of the original event.
-- A saved start is unique across all heads, including uncertain HTTP outcomes.
CREATE TABLE owner_shared_budget_demand_decisions (
 demand_id TEXT NOT NULL REFERENCES owner_shared_demands(id),
 budget_digest TEXT NOT NULL CHECK(length(budget_digest)=64),
 execution_budget_payload TEXT NOT NULL CHECK(json_valid(execution_budget_payload)),
 disposition TEXT NOT NULL CHECK(disposition IN ('start_prepared','budget_blocked')),
 start_intent_id TEXT UNIQUE REFERENCES owner_shared_budget_start_intents(id),
 input_payload TEXT NOT NULL CHECK(json_valid(input_payload) AND length(CAST(input_payload AS BLOB))<=16384),
 input_digest TEXT NOT NULL CHECK(length(input_digest)=64),
 ledger_revision INTEGER NOT NULL UNIQUE CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991),
 PRIMARY KEY(demand_id,budget_digest),
 CHECK((disposition='start_prepared')=(start_intent_id IS NOT NULL))
) STRICT;
CREATE UNIQUE INDEX owner_shared_budget_demand_one_start ON owner_shared_budget_demand_decisions(demand_id) WHERE start_intent_id IS NOT NULL;
CREATE TRIGGER owner_shared_budget_demand_decisions_immutable BEFORE UPDATE ON owner_shared_budget_demand_decisions BEGIN SELECT RAISE(ABORT,'immutable budget demand decision'); END;
CREATE TRIGGER owner_shared_budget_demand_decisions_retained BEFORE DELETE ON owner_shared_budget_demand_decisions BEGIN SELECT RAISE(ABORT,'retained budget demand decision'); END;
