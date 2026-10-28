-- Schema103: retain longer completed answers with the same immutable evidence.
-- Copy exact historical bytes/digests and preserve dependent refusal references.
PRAGMA defer_foreign_keys=ON;
CREATE TABLE owner_shared_work_observations_long_reply (
 id TEXT PRIMARY KEY,
 work_id TEXT NOT NULL REFERENCES owner_shared_work_inbox(id),
 payload TEXT NOT NULL CHECK(json_valid(payload) AND length(CAST(payload AS BLOB))<=49152),
 digest TEXT NOT NULL CHECK(length(digest)=64),
 ledger_revision INTEGER NOT NULL UNIQUE CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991)
) STRICT;
INSERT INTO owner_shared_work_observations_long_reply SELECT id,work_id,payload,digest,ledger_revision FROM owner_shared_work_observations;
DROP TABLE owner_shared_work_observations;
ALTER TABLE owner_shared_work_observations_long_reply RENAME TO owner_shared_work_observations;
CREATE TRIGGER owner_shared_work_observations_immutable BEFORE UPDATE ON owner_shared_work_observations BEGIN SELECT RAISE(ABORT,'immutable shared execution history');END;
CREATE TRIGGER owner_shared_work_observations_retained BEFORE DELETE ON owner_shared_work_observations BEGIN SELECT RAISE(ABORT,'retained shared execution history');END;
PRAGMA defer_foreign_keys=OFF;
