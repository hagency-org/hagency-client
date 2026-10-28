-- Isolated wire-pending demand journal. These descriptive rows never confer
-- process, activity or model-call authority.
CREATE TABLE owner_shared_demand_controls (
 child_id TEXT PRIMARY KEY REFERENCES owner_shared_runtime_lifetimes(child_id),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 control_revision INTEGER NOT NULL CHECK(control_revision>0 AND control_revision<=9007199254740991),
 stopped_through INTEGER NOT NULL DEFAULT 0 CHECK(stopped_through>=0 AND stopped_through<=9007199254740991)
) STRICT;
CREATE TRIGGER owner_shared_demand_controls_monotonic BEFORE UPDATE ON owner_shared_demand_controls WHEN NEW.child_id!=OLD.child_id OR NEW.profile!=OLD.profile OR NEW.control_revision<OLD.control_revision OR NEW.stopped_through<OLD.stopped_through BEGIN SELECT RAISE(ABORT,'monotonic demand control'); END;
CREATE TRIGGER owner_shared_demand_controls_retained BEFORE DELETE ON owner_shared_demand_controls BEGIN SELECT RAISE(ABORT,'retained demand control'); END;
CREATE TABLE owner_shared_demands (
 id TEXT PRIMARY KEY,
 child_id TEXT NOT NULL REFERENCES owner_shared_runtime_lifetimes(child_id),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 device_id TEXT NOT NULL,
 device_generation INTEGER NOT NULL CHECK(device_generation>0 AND device_generation<=9007199254740991),
 event_id TEXT NOT NULL,
 offer_payload TEXT NOT NULL CHECK(json_valid(offer_payload) AND length(CAST(offer_payload AS BLOB))<=8192),
 offer_digest TEXT NOT NULL CHECK(length(offer_digest)=64),
 ledger_revision INTEGER NOT NULL CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991),
 UNIQUE(child_id,event_id)
) STRICT;
CREATE TRIGGER owner_shared_demands_immutable BEFORE UPDATE ON owner_shared_demands BEGIN SELECT RAISE(ABORT,'immutable demand'); END;
CREATE TRIGGER owner_shared_demands_retained BEFORE DELETE ON owner_shared_demands BEGIN SELECT RAISE(ABORT,'retained demand'); END;
CREATE TABLE owner_shared_demand_decisions (
 demand_id TEXT PRIMARY KEY REFERENCES owner_shared_demands(id),
 disposition TEXT NOT NULL CHECK(disposition IN ('start_prepared','budget_blocked','head_changed')),
 start_intent_id TEXT UNIQUE REFERENCES owner_shared_start_intents(id),
 input_payload TEXT CHECK(input_payload IS NULL OR (json_valid(input_payload) AND length(CAST(input_payload AS BLOB))<=16384)),
 input_digest TEXT CHECK(input_digest IS NULL OR length(input_digest)=64),
 budget_payload TEXT CHECK(budget_payload IS NULL OR (json_valid(budget_payload) AND length(CAST(budget_payload AS BLOB))<=8192)),
 ledger_revision INTEGER NOT NULL CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991),
 CHECK((disposition='start_prepared' AND start_intent_id IS NOT NULL AND input_payload IS NOT NULL AND input_digest IS NOT NULL AND budget_payload IS NULL) OR (disposition='budget_blocked' AND start_intent_id IS NULL AND input_payload IS NULL AND input_digest IS NULL AND budget_payload IS NOT NULL) OR (disposition='head_changed' AND start_intent_id IS NULL AND input_payload IS NULL AND input_digest IS NULL AND budget_payload IS NULL))
) STRICT;
CREATE TRIGGER owner_shared_demand_decisions_immutable BEFORE UPDATE ON owner_shared_demand_decisions BEGIN SELECT RAISE(ABORT,'immutable demand disposition'); END;
CREATE TRIGGER owner_shared_demand_decisions_retained BEFORE DELETE ON owner_shared_demand_decisions BEGIN SELECT RAISE(ABORT,'retained demand disposition'); END;
CREATE TABLE owner_shared_demand_bindings (
 demand_id TEXT PRIMARY KEY REFERENCES owner_shared_demands(id),
 work_id TEXT NOT NULL UNIQUE,
 binding_payload TEXT NOT NULL CHECK(json_valid(binding_payload) AND length(CAST(binding_payload AS BLOB))<=16384),
 binding_digest TEXT NOT NULL CHECK(length(binding_digest)=64),
 ledger_revision INTEGER NOT NULL CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991)
) STRICT;
CREATE TRIGGER owner_shared_demand_bindings_immutable BEFORE UPDATE ON owner_shared_demand_bindings BEGIN SELECT RAISE(ABORT,'immutable demand work binding'); END;
CREATE TRIGGER owner_shared_demand_bindings_retained BEFORE DELETE ON owner_shared_demand_bindings BEGIN SELECT RAISE(ABORT,'retained demand work binding'); END;
