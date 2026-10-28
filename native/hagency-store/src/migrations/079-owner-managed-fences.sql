-- Schema77: exact permanent semantic tombstones, including cleanup delivered
-- before the original reserve. No process, usage or release proof is invented.
CREATE TABLE owner_managed_fences (
 profile TEXT NOT NULL CHECK(length(profile)=64),
 agent_id TEXT NOT NULL,
 request_id TEXT NOT NULL,
 original_command_id TEXT NOT NULL UNIQUE,
 original_command_digest TEXT NOT NULL CHECK(length(original_command_digest)=64),
 original_command TEXT NOT NULL CHECK(json_valid(original_command)),
 project_reservation_id TEXT NOT NULL REFERENCES owner_project_reservations(id),
 executor_device_id TEXT NOT NULL,
 fence_revision INTEGER NOT NULL CHECK(fence_revision>0 AND fence_revision<=9007199254740991),
 PRIMARY KEY(profile,agent_id),
 UNIQUE(profile,request_id)
) STRICT;
CREATE TRIGGER owner_managed_fence_immutable BEFORE UPDATE ON owner_managed_fences
BEGIN SELECT RAISE(ABORT,'permanent managed agent fence'); END;
CREATE TRIGGER owner_managed_fence_retained BEFORE DELETE ON owner_managed_fences
BEGIN SELECT RAISE(ABORT,'managed fence history is retained'); END;
