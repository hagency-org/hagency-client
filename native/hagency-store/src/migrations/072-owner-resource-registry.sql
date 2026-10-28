-- Identity/access/receipts live beside existing resources and reservation math.
-- No Matrix profile owns the installation's physical provider budget.
CREATE TABLE owner_resource_identity (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 id TEXT NOT NULL CHECK(length(id)=64),
 root_identity TEXT NOT NULL,
 revision INTEGER NOT NULL CHECK(revision>0)
);
CREATE TABLE owner_resource_accounts (
 provider_digest TEXT PRIMARY KEY CHECK(length(provider_digest)=64),
 seat_id TEXT NOT NULL UNIQUE REFERENCES seats(id),
 identity_kind TEXT NOT NULL CHECK(identity_kind IN ('verified_provider','explicit_owner_mapping'))
);
CREATE TABLE owner_resource_grants (
 profile TEXT NOT NULL CHECK(length(profile)=64),
 resource_id TEXT NOT NULL REFERENCES resources(id),
 revision INTEGER NOT NULL CHECK(revision>0),
 active INTEGER NOT NULL CHECK(active IN (0,1)),
 PRIMARY KEY(profile,resource_id)
);
CREATE TABLE owner_resource_receipts (
 command_id TEXT PRIMARY KEY,
 digest TEXT NOT NULL CHECK(length(digest)=64),
 result TEXT NOT NULL
);
