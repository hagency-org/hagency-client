-- These holds extend the existing resource/seat ledger. A project consumes
-- its parent once; it is never charged a second time to the provider account.
CREATE TABLE owner_capacity_grants (
 id TEXT PRIMARY KEY,
 profile TEXT NOT NULL CHECK(length(profile)=64),
 resource_id TEXT NOT NULL REFERENCES resources(id),
 preset_id TEXT NOT NULL,
 seat_id TEXT NOT NULL REFERENCES seats(id),
 allocated_tokens INTEGER NOT NULL CHECK(allocated_tokens>0),
 receipt TEXT NOT NULL,
 active INTEGER NOT NULL CHECK(active IN(0,1))
);
CREATE TABLE owner_project_reservations (
 id TEXT PRIMARY KEY,
 grant_id TEXT NOT NULL REFERENCES owner_capacity_grants(id),
 project_id TEXT NOT NULL,
 project_revision INTEGER NOT NULL CHECK(project_revision>0),
 reserve_command_id TEXT NOT NULL,
 reserve_digest TEXT,
 requested_tokens INTEGER NOT NULL CHECK(requested_tokens>0),
 retained_tokens INTEGER NOT NULL CHECK(retained_tokens>=0 AND retained_tokens<=requested_tokens),
 state TEXT NOT NULL CHECK(state IN('reserved','released')),
 binding_digest TEXT NOT NULL CHECK(length(binding_digest)=64),
 UNIQUE(grant_id,project_id,project_revision)
);
CREATE TABLE owner_capacity_receipts (
 command_id TEXT PRIMARY KEY,
 profile TEXT NOT NULL CHECK(length(profile)=64),
 executor_device_id TEXT NOT NULL,
 digest TEXT NOT NULL CHECK(length(digest)=64),
 result TEXT NOT NULL
);
