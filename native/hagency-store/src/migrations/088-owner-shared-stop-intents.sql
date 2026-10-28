-- Explicit owner Stop custody is distinct from observed cancellation/control.
CREATE TABLE owner_shared_stop_intents (
 command_id TEXT PRIMARY KEY,
 child_id TEXT NOT NULL REFERENCES owner_shared_runtime_lifetimes(child_id),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 device_id TEXT NOT NULL,
 device_generation INTEGER NOT NULL CHECK(device_generation>0 AND device_generation<=9007199254740991),
 actor_mxid TEXT NOT NULL,
 assignment_id TEXT NOT NULL,
 selected_revision INTEGER NOT NULL CHECK(selected_revision>0 AND selected_revision<9007199254740991),
 input_payload TEXT NOT NULL CHECK(json_valid(input_payload) AND length(CAST(input_payload AS BLOB))<=2048),
 input_digest TEXT NOT NULL CHECK(length(input_digest)=64),
 ledger_revision INTEGER NOT NULL CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991),
 UNIQUE(profile,child_id,selected_revision)
) STRICT;
CREATE TRIGGER owner_shared_stop_intents_immutable BEFORE UPDATE ON owner_shared_stop_intents BEGIN SELECT RAISE(ABORT,'immutable owner Stop intent'); END;
CREATE TRIGGER owner_shared_stop_intents_retained BEFORE DELETE ON owner_shared_stop_intents BEGIN SELECT RAISE(ABORT,'retained owner Stop intent'); END;
CREATE TABLE owner_shared_stop_receipts (
 command_id TEXT PRIMARY KEY REFERENCES owner_shared_stop_intents(command_id),
 applied_revision INTEGER NOT NULL CHECK(applied_revision>0 AND applied_revision<=9007199254740991),
 ledger_revision INTEGER NOT NULL CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991)
) STRICT;
CREATE TRIGGER owner_shared_stop_receipts_immutable BEFORE UPDATE ON owner_shared_stop_receipts BEGIN SELECT RAISE(ABORT,'immutable owner Stop receipt'); END;
CREATE TRIGGER owner_shared_stop_receipts_retained BEFORE DELETE ON owner_shared_stop_receipts BEGIN SELECT RAISE(ABORT,'retained owner Stop receipt'); END;
