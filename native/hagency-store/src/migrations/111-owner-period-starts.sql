-- Epoch-pinned start intent identities preserve the exact old request while
-- allowing a new period after an unsubmitted prior-period intent. Process
-- claims still require the exact stopped predecessor and native call budget.
PRAGMA defer_foreign_keys=ON;
CREATE TABLE owner_shared_start_intents_period_migration (
 id TEXT PRIMARY KEY,
 child_id TEXT NOT NULL REFERENCES owner_shared_runtime_lifetimes(child_id),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 device_id TEXT NOT NULL,
 device_generation INTEGER NOT NULL CHECK(device_generation>0 AND device_generation<=9007199254740991),
 predecessor_digest TEXT NOT NULL CHECK(length(predecessor_digest)=64),
 original_runtime_digest TEXT NOT NULL CHECK(length(original_runtime_digest)=64),
 original_runtime_payload TEXT NOT NULL CHECK(json_valid(original_runtime_payload) AND length(CAST(original_runtime_payload AS BLOB))<=16384),
 predecessor_payload TEXT CHECK(predecessor_payload IS NULL OR (json_valid(predecessor_payload) AND length(CAST(predecessor_payload AS BLOB))<=32768)),
 input_payload TEXT NOT NULL CHECK(json_valid(input_payload) AND length(CAST(input_payload AS BLOB))<=8192),
 input_digest TEXT NOT NULL CHECK(length(input_digest)=64),
 ledger_revision INTEGER NOT NULL UNIQUE CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991),
 period_id TEXT REFERENCES owner_managed_periods(id)
) STRICT;
INSERT INTO owner_shared_start_intents_period_migration(id,child_id,profile,device_id,device_generation,predecessor_digest,original_runtime_digest,original_runtime_payload,predecessor_payload,input_payload,input_digest,ledger_revision,period_id)
 SELECT s.id,s.child_id,s.profile,s.device_id,s.device_generation,s.predecessor_digest,s.original_runtime_digest,s.original_runtime_payload,s.predecessor_payload,s.input_payload,s.input_digest,s.ledger_revision,
 (SELECT p.id FROM owner_managed_periods p WHERE p.child_id=s.child_id AND p.ledger_revision<=s.ledger_revision ORDER BY p.ledger_revision DESC LIMIT 1)
 FROM owner_shared_start_intents s;
DROP TABLE owner_shared_start_intents;
ALTER TABLE owner_shared_start_intents_period_migration RENAME TO owner_shared_start_intents;
CREATE UNIQUE INDEX owner_shared_start_intent_period ON owner_shared_start_intents(child_id,predecessor_digest,COALESCE(period_id,''));
CREATE TRIGGER owner_shared_start_intents_immutable BEFORE UPDATE ON owner_shared_start_intents BEGIN SELECT RAISE(ABORT,'immutable shared start intent');END;
CREATE TRIGGER owner_shared_start_intents_retained BEFORE DELETE ON owner_shared_start_intents BEGIN SELECT RAISE(ABORT,'retained shared start intent');END;
CREATE TEMP TABLE period_start_migration_check(valid INTEGER NOT NULL CHECK(valid=1));
INSERT INTO period_start_migration_check SELECT NOT EXISTS(SELECT 1 FROM pragma_foreign_key_check);
DROP TABLE period_start_migration_check;
PRAGMA defer_foreign_keys=OFF;
