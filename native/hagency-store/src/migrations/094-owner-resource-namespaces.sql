-- Canonical quota identity and private execution namespace are independent.
CREATE TABLE owner_resource_namespaces (
 resource_id TEXT PRIMARY KEY REFERENCES resources(id),
 account_id TEXT NOT NULL REFERENCES managed_accounts(id),
 generation INTEGER NOT NULL CHECK(generation>0),
 profile TEXT NOT NULL CHECK(length(profile)=64)
) STRICT;
CREATE TRIGGER owner_resource_namespaces_immutable BEFORE UPDATE ON owner_resource_namespaces BEGIN SELECT RAISE(ABORT,'immutable resource namespace'); END;
CREATE TRIGGER owner_resource_namespaces_retained BEFORE DELETE ON owner_resource_namespaces BEGIN SELECT RAISE(ABORT,'retained resource namespace'); END;
CREATE TABLE owner_namespace_enrollments (
 command_id TEXT PRIMARY KEY REFERENCES owner_resource_receipts(command_id),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 input_digest TEXT NOT NULL CHECK(length(input_digest)=64),
 resource_id TEXT NOT NULL REFERENCES resources(id)
) STRICT;
CREATE TRIGGER owner_namespace_enrollments_immutable BEFORE UPDATE ON owner_namespace_enrollments BEGIN SELECT RAISE(ABORT,'immutable namespace enrollment'); END;
CREATE TRIGGER owner_namespace_enrollments_retained BEFORE DELETE ON owner_namespace_enrollments BEGIN SELECT RAISE(ABORT,'retained namespace enrollment'); END;
