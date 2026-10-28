-- Schema80: original text-process custody and private exact work journal.
-- These rows contain no bearer authority and never recreate process custody.
CREATE TABLE owner_shared_runtime_lifetimes (
 child_id TEXT PRIMARY KEY REFERENCES owner_managed_agents(id),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 assignment_id TEXT NOT NULL UNIQUE,
 command_id TEXT NOT NULL UNIQUE,
 command_payload TEXT NOT NULL CHECK(json_valid(command_payload) AND length(CAST(command_payload AS BLOB))<=16384),
 command_digest TEXT NOT NULL CHECK(length(command_digest)=64),
 binding_digest TEXT NOT NULL CHECK(length(binding_digest)=64),
 provider_digest TEXT NOT NULL REFERENCES owner_resource_accounts(provider_digest),
 issued_revision INTEGER NOT NULL UNIQUE CHECK(issued_revision>0 AND issued_revision<=9007199254740991),
 incarnation TEXT NOT NULL UNIQUE
) STRICT;
CREATE TABLE owner_shared_process_observations (
 id TEXT PRIMARY KEY,
 child_id TEXT NOT NULL REFERENCES owner_shared_runtime_lifetimes(child_id),
 payload TEXT NOT NULL CHECK(json_valid(payload) AND length(CAST(payload AS BLOB))<=16384),
 digest TEXT NOT NULL CHECK(length(digest)=64),
 ledger_revision INTEGER NOT NULL UNIQUE CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991)
) STRICT;
CREATE TABLE owner_shared_work_inbox (
 id TEXT PRIMARY KEY,
 child_id TEXT NOT NULL REFERENCES owner_shared_runtime_lifetimes(child_id),
 execution_id TEXT NOT NULL UNIQUE,
 payload TEXT NOT NULL CHECK(json_valid(payload) AND length(CAST(payload AS BLOB))<=16384),
 digest TEXT NOT NULL CHECK(length(digest)=64),
 received_revision INTEGER NOT NULL UNIQUE CHECK(received_revision>0 AND received_revision<=9007199254740991)
) STRICT;
CREATE TABLE owner_shared_work_events (
 id TEXT PRIMARY KEY,
 work_id TEXT NOT NULL REFERENCES owner_shared_work_inbox(id),
 kind TEXT NOT NULL CHECK(kind IN('start_requested','start_confirmed','start_uncertain','call_issued','reply_observed')),
 payload TEXT NOT NULL CHECK(json_valid(payload) AND length(CAST(payload AS BLOB))<=16384),
 digest TEXT NOT NULL CHECK(length(digest)=64),
 ledger_revision INTEGER NOT NULL UNIQUE CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991),
 UNIQUE(work_id,kind)
) STRICT;
CREATE TABLE owner_shared_work_observations (
 id TEXT PRIMARY KEY,
 work_id TEXT NOT NULL REFERENCES owner_shared_work_inbox(id),
 payload TEXT NOT NULL CHECK(json_valid(payload) AND length(CAST(payload AS BLOB))<=12288),
 digest TEXT NOT NULL CHECK(length(digest)=64),
 ledger_revision INTEGER NOT NULL UNIQUE CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991)
) STRICT;
CREATE TABLE owner_shared_activity_renewals (
 id TEXT PRIMARY KEY,
 child_id TEXT NOT NULL REFERENCES owner_shared_runtime_lifetimes(child_id),
 requested_epoch INTEGER NOT NULL CHECK(requested_epoch>0 AND requested_epoch<=9007199254740991),
 requested_revision INTEGER NOT NULL UNIQUE CHECK(requested_revision>0 AND requested_revision<=9007199254740991)
) STRICT;
CREATE TRIGGER owner_shared_excludes_metadata BEFORE INSERT ON owner_shared_runtime_lifetimes WHEN EXISTS(SELECT 1 FROM owner_managed_runtime_lifetimes WHERE child_id=NEW.child_id) BEGIN SELECT RAISE(ABORT,'managed runtime policy already pinned');END;
CREATE TRIGGER owner_metadata_excludes_shared BEFORE INSERT ON owner_managed_runtime_lifetimes WHEN EXISTS(SELECT 1 FROM owner_shared_runtime_lifetimes WHERE child_id=NEW.child_id) BEGIN SELECT RAISE(ABORT,'managed text policy already pinned');END;
CREATE TRIGGER owner_shared_runtime_lifetimes_immutable BEFORE UPDATE ON owner_shared_runtime_lifetimes BEGIN SELECT RAISE(ABORT,'immutable shared execution history');END;
CREATE TRIGGER owner_shared_runtime_lifetimes_retained BEFORE DELETE ON owner_shared_runtime_lifetimes BEGIN SELECT RAISE(ABORT,'retained shared execution history');END;
CREATE TRIGGER owner_shared_process_observations_immutable BEFORE UPDATE ON owner_shared_process_observations BEGIN SELECT RAISE(ABORT,'immutable shared execution history');END;
CREATE TRIGGER owner_shared_process_observations_retained BEFORE DELETE ON owner_shared_process_observations BEGIN SELECT RAISE(ABORT,'retained shared execution history');END;
CREATE TRIGGER owner_shared_work_inbox_immutable BEFORE UPDATE ON owner_shared_work_inbox BEGIN SELECT RAISE(ABORT,'immutable shared execution history');END;
CREATE TRIGGER owner_shared_work_inbox_retained BEFORE DELETE ON owner_shared_work_inbox BEGIN SELECT RAISE(ABORT,'retained shared execution history');END;
CREATE TRIGGER owner_shared_work_events_immutable BEFORE UPDATE ON owner_shared_work_events BEGIN SELECT RAISE(ABORT,'immutable shared execution history');END;
CREATE TRIGGER owner_shared_work_events_retained BEFORE DELETE ON owner_shared_work_events BEGIN SELECT RAISE(ABORT,'retained shared execution history');END;
CREATE TRIGGER owner_shared_work_observations_immutable BEFORE UPDATE ON owner_shared_work_observations BEGIN SELECT RAISE(ABORT,'immutable shared execution history');END;
CREATE TRIGGER owner_shared_work_observations_retained BEFORE DELETE ON owner_shared_work_observations BEGIN SELECT RAISE(ABORT,'retained shared execution history');END;
CREATE TRIGGER owner_shared_activity_renewals_immutable BEFORE UPDATE ON owner_shared_activity_renewals BEGIN SELECT RAISE(ABORT,'immutable shared execution history');END;
CREATE TRIGGER owner_shared_activity_renewals_retained BEFORE DELETE ON owner_shared_activity_renewals BEGIN SELECT RAISE(ABORT,'retained shared execution history');END;
