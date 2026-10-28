-- Schema 75: children partition a real owner project reservation. No legacy
-- registration/engagement or personal execution identity is manufactured.
CREATE TABLE owner_project_authorities (
 allocation_id TEXT PRIMARY KEY REFERENCES owner_project_reservations(id),
 reserve_command TEXT NOT NULL CHECK(json_valid(reserve_command)),
 reserve_digest TEXT NOT NULL CHECK(length(reserve_digest)=64),
 manager_mxid TEXT NOT NULL,
 definition_digest TEXT NOT NULL CHECK(length(definition_digest)=64)
) STRICT;
CREATE TABLE owner_managed_agents (
 id TEXT PRIMARY KEY,
 profile TEXT NOT NULL CHECK(length(profile)=64),
 agent_id TEXT NOT NULL,
 request_id TEXT NOT NULL,
 project_reservation_id TEXT NOT NULL REFERENCES owner_project_reservations(id),
 original_command_id TEXT NOT NULL UNIQUE,
 original_command_digest TEXT NOT NULL CHECK(length(original_command_digest)=64),
 original_command TEXT NOT NULL CHECK(json_valid(original_command)),
 definition_digest TEXT NOT NULL CHECK(length(definition_digest)=64),
 allocated_tokens INTEGER NOT NULL CHECK(allocated_tokens>0 AND allocated_tokens<=9007199254740991),
 budget_revision INTEGER NOT NULL CHECK(budget_revision>0 AND budget_revision<=9007199254740991),
 state TEXT NOT NULL CHECK(state IN('reserved','fenced')),
 permanent_fence INTEGER NOT NULL DEFAULT 0 CHECK(permanent_fence IN(0,1)),
 CHECK((state='fenced')=(permanent_fence=1)),
 UNIQUE(profile,agent_id),
 UNIQUE(profile,request_id)
) STRICT;
CREATE INDEX owner_managed_project_children ON owner_managed_agents(project_reservation_id,id);
CREATE TABLE owner_managed_effects (
 id TEXT PRIMARY KEY,
 child_id TEXT NOT NULL UNIQUE REFERENCES owner_managed_agents(id),
 kind TEXT NOT NULL CHECK(kind='provision'),
 state TEXT NOT NULL CHECK(state IN('pending','issued','qualified','failed','unknown')),
 fence INTEGER NOT NULL CHECK(fence>=0 AND fence<=9007199254740991),
 payload_digest TEXT NOT NULL CHECK(length(payload_digest)=64),
 original_result TEXT CHECK(original_result IS NULL OR json_valid(original_result)),
 original_result_digest TEXT CHECK(original_result_digest IS NULL OR length(original_result_digest)=64)
) STRICT;
-- No production call issuer is connected in the reservation checkpoint. These
-- rows will be owned only by a native one-shot custody adapter, never a script.
CREATE TABLE owner_managed_calls (
 id TEXT PRIMARY KEY,
 child_id TEXT NOT NULL REFERENCES owner_managed_agents(id),
 execution_id TEXT NOT NULL,
 immutable_digest TEXT NOT NULL CHECK(length(immutable_digest)=64),
 assignment_generation INTEGER NOT NULL CHECK(assignment_generation>0 AND assignment_generation<=9007199254740991),
 state TEXT NOT NULL CHECK(state IN('prepared','issued','settled','unknown','cancelled_before_issue')),
 reserved_tokens INTEGER NOT NULL CHECK(reserved_tokens>=0 AND reserved_tokens<=9007199254740991),
 known_consumed_tokens INTEGER NOT NULL CHECK(known_consumed_tokens>=0 AND known_consumed_tokens<=9007199254740991),
 usage_complete INTEGER NOT NULL CHECK(usage_complete IN(0,1)),
 unrepresentable_usage INTEGER NOT NULL DEFAULT 0 CHECK(unrepresentable_usage IN(0,1)),
 original_result_digest TEXT CHECK(original_result_digest IS NULL OR length(original_result_digest)=64),
 UNIQUE(child_id,execution_id)
) STRICT;
CREATE INDEX owner_managed_child_calls ON owner_managed_calls(child_id,id);
CREATE TRIGGER owner_project_authority_immutable BEFORE UPDATE ON owner_project_authorities
BEGIN SELECT RAISE(ABORT,'immutable original project authority'); END;
CREATE TRIGGER owner_project_authority_retained BEFORE DELETE ON owner_project_authorities
BEGIN SELECT RAISE(ABORT,'original project authority is retained'); END;
CREATE TRIGGER owner_managed_agent_binding_immutable BEFORE UPDATE ON owner_managed_agents
WHEN NEW.id IS NOT OLD.id OR NEW.profile IS NOT OLD.profile OR NEW.agent_id IS NOT OLD.agent_id
 OR NEW.request_id IS NOT OLD.request_id OR NEW.project_reservation_id IS NOT OLD.project_reservation_id
 OR NEW.original_command_id IS NOT OLD.original_command_id OR NEW.original_command_digest IS NOT OLD.original_command_digest
 OR NEW.original_command IS NOT OLD.original_command OR NEW.definition_digest IS NOT OLD.definition_digest
 OR NEW.allocated_tokens IS NOT OLD.allocated_tokens OR NEW.budget_revision IS NOT OLD.budget_revision
 OR NEW.permanent_fence<OLD.permanent_fence
BEGIN SELECT RAISE(ABORT,'immutable managed agent binding'); END;

CREATE TRIGGER owner_managed_agent_retained BEFORE DELETE ON owner_managed_agents
BEGIN SELECT RAISE(ABORT,'managed agent history is retained'); END;
CREATE TRIGGER owner_managed_effect_binding_immutable BEFORE UPDATE ON owner_managed_effects
WHEN NEW.id IS NOT OLD.id OR NEW.child_id IS NOT OLD.child_id OR NEW.kind IS NOT OLD.kind
 OR NEW.payload_digest IS NOT OLD.payload_digest OR NEW.fence<OLD.fence
 OR (OLD.original_result IS NOT NULL AND NEW.original_result IS NOT OLD.original_result)
 OR (OLD.original_result_digest IS NOT NULL AND NEW.original_result_digest IS NOT OLD.original_result_digest)
BEGIN SELECT RAISE(ABORT,'immutable managed effect evidence'); END;
CREATE TRIGGER owner_managed_effect_retained BEFORE DELETE ON owner_managed_effects
BEGIN SELECT RAISE(ABORT,'managed effect history is retained'); END;
CREATE TRIGGER owner_managed_call_binding_immutable BEFORE UPDATE ON owner_managed_calls
WHEN NEW.id IS NOT OLD.id OR NEW.child_id IS NOT OLD.child_id OR NEW.execution_id IS NOT OLD.execution_id
 OR NEW.immutable_digest IS NOT OLD.immutable_digest OR NEW.assignment_generation IS NOT OLD.assignment_generation
 OR NEW.reserved_tokens IS NOT OLD.reserved_tokens OR NEW.known_consumed_tokens<OLD.known_consumed_tokens
 OR NEW.unrepresentable_usage<OLD.unrepresentable_usage
 OR (OLD.original_result_digest IS NOT NULL AND NEW.original_result_digest IS NOT OLD.original_result_digest)
BEGIN SELECT RAISE(ABORT,'immutable managed call evidence'); END;
CREATE TRIGGER owner_managed_call_retained BEFORE DELETE ON owner_managed_calls
BEGIN SELECT RAISE(ABORT,'managed usage history is retained'); END;
