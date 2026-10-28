-- Schema76: native-only, one-shot effect custody and immutable observation
-- receipts. No production transport constructs the required authority yet.
CREATE TABLE owner_managed_assignment_pins (
 child_id TEXT PRIMARY KEY REFERENCES owner_managed_agents(id),
 assignment_id TEXT NOT NULL,
 assignment_generation INTEGER NOT NULL CHECK(assignment_generation>0 AND assignment_generation<=9007199254740991),
 binding_digest TEXT NOT NULL CHECK(length(binding_digest)=64),
 provider_digest TEXT NOT NULL REFERENCES owner_resource_accounts(provider_digest)
) STRICT;
CREATE TABLE owner_managed_call_pins (
 call_id TEXT PRIMARY KEY REFERENCES owner_managed_calls(id),
 assignment_digest TEXT NOT NULL CHECK(length(assignment_digest)=64),
 execution_digest TEXT NOT NULL CHECK(length(execution_digest)=64),
 prepare_revision INTEGER NOT NULL CHECK(prepare_revision>0 AND prepare_revision<=9007199254740991),
 issue_revision INTEGER CHECK(issue_revision IS NULL OR (issue_revision>prepare_revision AND issue_revision<=9007199254740991))
) STRICT;
CREATE TABLE owner_managed_observations (
 id TEXT PRIMARY KEY,
 child_id TEXT NOT NULL REFERENCES owner_managed_agents(id),
 target_id TEXT NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN('provision','usage','fence')),
 content_digest TEXT NOT NULL CHECK(length(content_digest)=64),
 evidence TEXT NOT NULL CHECK(json_valid(evidence) AND length(evidence)<=16384),
 result TEXT NOT NULL CHECK(json_valid(result)),
 ledger_revision INTEGER NOT NULL CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991)
) STRICT;
CREATE INDEX owner_managed_observation_history ON owner_managed_observations(child_id,ledger_revision);
CREATE TRIGGER owner_managed_assignment_immutable BEFORE UPDATE ON owner_managed_assignment_pins
BEGIN SELECT RAISE(ABORT,'immutable managed assignment pin'); END;
CREATE TRIGGER owner_managed_assignment_retained BEFORE DELETE ON owner_managed_assignment_pins
BEGIN SELECT RAISE(ABORT,'managed assignment history is retained'); END;
CREATE TRIGGER owner_managed_call_pin_immutable BEFORE UPDATE ON owner_managed_call_pins
WHEN NEW.call_id IS NOT OLD.call_id OR NEW.assignment_digest IS NOT OLD.assignment_digest
 OR NEW.execution_digest IS NOT OLD.execution_digest OR NEW.prepare_revision IS NOT OLD.prepare_revision
 OR (OLD.issue_revision IS NOT NULL AND NEW.issue_revision IS NOT OLD.issue_revision)
BEGIN SELECT RAISE(ABORT,'immutable managed call pin'); END;
CREATE TRIGGER owner_managed_call_pin_retained BEFORE DELETE ON owner_managed_call_pins
BEGIN SELECT RAISE(ABORT,'managed call pin is retained'); END;
CREATE TRIGGER owner_managed_observation_immutable BEFORE UPDATE ON owner_managed_observations
BEGIN SELECT RAISE(ABORT,'immutable managed observation'); END;
CREATE TRIGGER owner_managed_observation_retained BEFORE DELETE ON owner_managed_observations
BEGIN SELECT RAISE(ABORT,'managed observation is retained'); END;
