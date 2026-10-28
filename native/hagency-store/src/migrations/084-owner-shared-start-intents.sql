-- A native accepted startup persists its exact request before the first HTTP
-- POST. Polling/recovery cannot mint another intent for the same predecessor.
CREATE TABLE owner_shared_start_intents (
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
 UNIQUE(child_id,predecessor_digest)
) STRICT;
CREATE TRIGGER owner_shared_start_intents_immutable BEFORE UPDATE ON owner_shared_start_intents BEGIN SELECT RAISE(ABORT,'immutable shared start intent');END;
CREATE TRIGGER owner_shared_start_intents_retained BEFORE DELETE ON owner_shared_start_intents BEGIN SELECT RAISE(ABORT,'retained shared start intent');END;
