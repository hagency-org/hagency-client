-- Schema79: one immutable central provision command per native child. Issued
-- history never grants another process. Stop is a permanent semantic tombstone.
CREATE TABLE owner_managed_runtime_lifetimes (
 child_id TEXT PRIMARY KEY REFERENCES owner_managed_agents(id),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 assignment_id TEXT NOT NULL,
 command_id TEXT NOT NULL UNIQUE,
 command_payload TEXT NOT NULL CHECK(json_valid(command_payload) AND length(command_payload)<=16384),
 command_digest TEXT NOT NULL CHECK(length(command_digest)=64),
 binding_digest TEXT NOT NULL CHECK(length(binding_digest)=64),
 provider_digest TEXT NOT NULL REFERENCES owner_resource_accounts(provider_digest),
 issued_revision INTEGER CHECK(issued_revision IS NULL OR (issued_revision>0 AND issued_revision<=9007199254740991)),
 incarnation TEXT,
 source_effect_state TEXT NOT NULL CHECK(source_effect_state IN('pending','issued','qualified','failed','unknown')),
 CHECK((issued_revision IS NULL)=(incarnation IS NULL))
) STRICT;
CREATE TABLE owner_managed_runtime_stops (
 child_id TEXT PRIMARY KEY REFERENCES owner_managed_runtime_lifetimes(child_id),
 command_id TEXT NOT NULL UNIQUE,
 command_payload TEXT NOT NULL CHECK(json_valid(command_payload) AND length(command_payload)<=16384),
 command_digest TEXT NOT NULL CHECK(length(command_digest)=64),
 fence_revision INTEGER NOT NULL CHECK(fence_revision>0 AND fence_revision<=9007199254740991)
) STRICT;
CREATE TABLE owner_managed_runtime_observations (
 id TEXT PRIMARY KEY,
 child_id TEXT NOT NULL REFERENCES owner_managed_runtime_lifetimes(child_id),
 command_id TEXT NOT NULL,
 payload TEXT NOT NULL CHECK(json_valid(payload) AND length(payload)<=16384),
 digest TEXT NOT NULL CHECK(length(digest)=64),
 ledger_revision INTEGER NOT NULL UNIQUE CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991)
) STRICT;
CREATE INDEX owner_managed_runtime_history ON owner_managed_runtime_observations(child_id,ledger_revision);
CREATE TRIGGER owner_managed_runtime_lifetime_immutable BEFORE UPDATE ON owner_managed_runtime_lifetimes BEGIN SELECT RAISE(ABORT,'immutable original managed runtime');END;
CREATE TRIGGER owner_managed_runtime_lifetime_retained BEFORE DELETE ON owner_managed_runtime_lifetimes BEGIN SELECT RAISE(ABORT,'retained managed runtime history');END;
CREATE TRIGGER owner_managed_runtime_stop_immutable BEFORE UPDATE ON owner_managed_runtime_stops BEGIN SELECT RAISE(ABORT,'permanent managed runtime stop');END;
CREATE TRIGGER owner_managed_runtime_stop_retained BEFORE DELETE ON owner_managed_runtime_stops BEGIN SELECT RAISE(ABORT,'retained managed runtime stop');END;
CREATE TRIGGER owner_managed_runtime_observation_immutable BEFORE UPDATE ON owner_managed_runtime_observations BEGIN SELECT RAISE(ABORT,'immutable managed runtime observation');END;
CREATE TRIGGER owner_managed_runtime_observation_retained BEFORE DELETE ON owner_managed_runtime_observations BEGIN SELECT RAISE(ABORT,'retained managed runtime observation');END;
