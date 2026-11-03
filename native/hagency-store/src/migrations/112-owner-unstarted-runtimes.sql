-- Definitive non-issuance of an expired recurring initial runtime command.
-- No process identity, effect issuance or Agent fence is manufactured here.
CREATE TABLE owner_shared_unstarted_runtimes (
 command_id TEXT PRIMARY KEY,
 child_id TEXT NOT NULL REFERENCES owner_managed_agents(id),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 command_payload TEXT NOT NULL CHECK(json_valid(command_payload) AND length(CAST(command_payload AS BLOB))<=16384),
 command_digest TEXT NOT NULL CHECK(length(command_digest)=64),
 receipt TEXT NOT NULL CHECK(json_valid(receipt) AND length(CAST(receipt AS BLOB))<=16384),
 receipt_digest TEXT NOT NULL CHECK(length(receipt_digest)=64),
 ledger_revision INTEGER NOT NULL UNIQUE CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991)
) STRICT;
CREATE INDEX owner_shared_unstarted_runtime_child ON owner_shared_unstarted_runtimes(child_id);
CREATE TRIGGER owner_shared_unstarted_runtimes_immutable BEFORE UPDATE ON owner_shared_unstarted_runtimes BEGIN SELECT RAISE(ABORT,'immutable initial runtime non-issuance');END;
CREATE TRIGGER owner_shared_unstarted_runtimes_retained BEFORE DELETE ON owner_shared_unstarted_runtimes BEGIN SELECT RAISE(ABORT,'retained initial runtime non-issuance');END;
