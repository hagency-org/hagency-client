-- Schema81: additive process attempts. Original v1 lifetime/work/receipt bytes
-- are retained. No row recreates the one-shot process right returned by claim.
CREATE TABLE owner_shared_successor_attempts (
 id TEXT PRIMARY KEY,
 child_id TEXT NOT NULL REFERENCES owner_shared_runtime_lifetimes(child_id),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 process_attempt INTEGER NOT NULL CHECK(process_attempt>=2 AND process_attempt<=9007199254740991),
 predecessor_id TEXT NOT NULL,
 predecessor_digest TEXT NOT NULL CHECK(length(predecessor_digest)=64),
 predecessor_stop_digest TEXT NOT NULL CHECK(length(predecessor_stop_digest)=64),
 start_intent_id TEXT NOT NULL,
 command_payload TEXT NOT NULL CHECK(json_valid(command_payload) AND length(CAST(command_payload AS BLOB))<=32768),
 command_digest TEXT NOT NULL UNIQUE CHECK(length(command_digest)=64),
 original_runtime_digest TEXT NOT NULL CHECK(length(original_runtime_digest)=64),
 binding_digest TEXT NOT NULL CHECK(length(binding_digest)=64),
 provider_digest TEXT NOT NULL REFERENCES owner_resource_accounts(provider_digest),
 observed_revision INTEGER NOT NULL CHECK(observed_revision>0 AND observed_revision<=9007199254740991),
 issued_revision INTEGER NOT NULL UNIQUE CHECK(issued_revision>observed_revision AND issued_revision<=9007199254740991),
 incarnation TEXT NOT NULL UNIQUE,
 UNIQUE(child_id,process_attempt),
 UNIQUE(child_id,predecessor_id),
 UNIQUE(child_id,start_intent_id)
) STRICT;
CREATE TABLE owner_shared_successor_observations (
 id TEXT PRIMARY KEY,
 attempt_id TEXT NOT NULL REFERENCES owner_shared_successor_attempts(id),
 payload TEXT NOT NULL CHECK(json_valid(payload) AND length(CAST(payload AS BLOB))<=16384),
 digest TEXT NOT NULL CHECK(length(digest)=64),
 ledger_revision INTEGER NOT NULL UNIQUE CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991)
) STRICT;
CREATE TABLE owner_shared_successor_call_pins (
 call_id TEXT PRIMARY KEY REFERENCES owner_managed_calls(id),
 attempt_id TEXT NOT NULL REFERENCES owner_shared_successor_attempts(id),
 work_id TEXT NOT NULL UNIQUE REFERENCES owner_shared_work_inbox(id),
 execution_digest TEXT NOT NULL CHECK(length(execution_digest)=64)
) STRICT;
CREATE TRIGGER owner_shared_successor_attempts_immutable BEFORE UPDATE ON owner_shared_successor_attempts BEGIN SELECT RAISE(ABORT,'immutable successor custody');END;
CREATE TRIGGER owner_shared_successor_attempts_retained BEFORE DELETE ON owner_shared_successor_attempts BEGIN SELECT RAISE(ABORT,'retained successor custody');END;
CREATE TRIGGER owner_shared_successor_observations_immutable BEFORE UPDATE ON owner_shared_successor_observations BEGIN SELECT RAISE(ABORT,'immutable successor observation');END;
CREATE TRIGGER owner_shared_successor_observations_retained BEFORE DELETE ON owner_shared_successor_observations BEGIN SELECT RAISE(ABORT,'retained successor observation');END;
CREATE TRIGGER owner_shared_successor_call_pins_immutable BEFORE UPDATE ON owner_shared_successor_call_pins BEGIN SELECT RAISE(ABORT,'immutable successor call binding');END;
CREATE TRIGGER owner_shared_successor_call_pins_retained BEFORE DELETE ON owner_shared_successor_call_pins BEGIN SELECT RAISE(ABORT,'retained successor call binding');END;
-- A permanent process-only cancellation can precede the one-shot claim. It is
-- not a process observation and releases no retained child or call liability.
CREATE TABLE owner_shared_successor_fences (
 command_id TEXT PRIMARY KEY,
 child_id TEXT NOT NULL REFERENCES owner_shared_runtime_lifetimes(child_id),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 process_attempt INTEGER NOT NULL CHECK(process_attempt>=2 AND process_attempt<=9007199254740991),
 predecessor_id TEXT NOT NULL,
 start_intent_id TEXT NOT NULL,
 command_payload TEXT NOT NULL CHECK(json_valid(command_payload) AND length(CAST(command_payload AS BLOB))<=32768),
 command_digest TEXT NOT NULL UNIQUE CHECK(length(command_digest)=64),
 fence_revision INTEGER NOT NULL UNIQUE CHECK(fence_revision>0 AND fence_revision<=9007199254740991),
 claimed_revision INTEGER CHECK(claimed_revision>0 AND claimed_revision<fence_revision),
 UNIQUE(child_id,process_attempt),
 UNIQUE(child_id,predecessor_id),
 UNIQUE(child_id,start_intent_id)
) STRICT;
CREATE TRIGGER owner_shared_successor_fences_immutable BEFORE UPDATE ON owner_shared_successor_fences BEGIN SELECT RAISE(ABORT,'immutable successor fence');END;
CREATE TRIGGER owner_shared_successor_fences_retained BEFORE DELETE ON owner_shared_successor_fences BEGIN SELECT RAISE(ABORT,'retained successor fence');END;
CREATE TRIGGER owner_shared_successor_claim_fence BEFORE INSERT ON owner_shared_successor_attempts WHEN EXISTS(SELECT 1 FROM owner_shared_successor_fences f WHERE f.child_id=NEW.child_id AND (f.process_attempt=NEW.process_attempt OR f.command_id=NEW.id OR f.start_intent_id=NEW.start_intent_id OR f.predecessor_id=NEW.predecessor_id)) BEGIN SELECT RAISE(ABORT,'successor permanently fenced');END;
