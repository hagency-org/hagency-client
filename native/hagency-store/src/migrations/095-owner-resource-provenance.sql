-- Absence is unknown history, never proof that an existing resource is unused.
-- New creation evidence commits with its original enrollment receipt. Exposure
-- is irreversible and shared by every profile using this physical resource.
CREATE TABLE owner_resource_provenance (
 resource_id TEXT PRIMARY KEY REFERENCES resources(id),
 registry_id TEXT NOT NULL CHECK(length(registry_id)=64),
 creation_command_id TEXT REFERENCES owner_resource_receipts(command_id),
 creation_digest TEXT CHECK(creation_digest IS NULL OR length(creation_digest)=64),
 creation_profile TEXT CHECK(creation_profile IS NULL OR length(creation_profile)=64),
 first_offer_kind TEXT CHECK(first_offer_kind IS NULL OR first_offer_kind IN ('contribution','capacity_grant','publication')),
 first_offer_profile TEXT CHECK(first_offer_profile IS NULL OR length(first_offer_profile)=64),
 first_offer_command_id TEXT,
 first_offer_revision TEXT CHECK(first_offer_revision IS NULL OR length(first_offer_revision)=64),
 first_offer_at_ms INTEGER CHECK(first_offer_at_ms IS NULL OR first_offer_at_ms>=0),
 CHECK((creation_command_id IS NULL AND creation_digest IS NULL AND creation_profile IS NULL)
    OR (creation_command_id IS NOT NULL AND creation_digest IS NOT NULL AND creation_profile IS NOT NULL)),
 CHECK((first_offer_kind IS NULL AND first_offer_profile IS NULL AND first_offer_command_id IS NULL
        AND first_offer_revision IS NULL AND first_offer_at_ms IS NULL)
    OR (first_offer_kind IS NOT NULL AND first_offer_profile IS NOT NULL AND first_offer_command_id IS NOT NULL
        AND first_offer_revision IS NOT NULL AND first_offer_at_ms IS NOT NULL))
);

CREATE TRIGGER owner_resource_provenance_immutable
BEFORE UPDATE ON owner_resource_provenance
WHEN OLD.resource_id IS NOT NEW.resource_id
 OR OLD.registry_id IS NOT NEW.registry_id
 OR OLD.creation_command_id IS NOT NEW.creation_command_id
 OR OLD.creation_digest IS NOT NEW.creation_digest
 OR OLD.creation_profile IS NOT NEW.creation_profile
 OR (OLD.first_offer_kind IS NOT NULL AND (
      OLD.first_offer_kind IS NOT NEW.first_offer_kind
   OR OLD.first_offer_profile IS NOT NEW.first_offer_profile
   OR OLD.first_offer_command_id IS NOT NEW.first_offer_command_id
   OR OLD.first_offer_revision IS NOT NEW.first_offer_revision
   OR OLD.first_offer_at_ms IS NOT NEW.first_offer_at_ms))
BEGIN SELECT RAISE(ABORT, 'resource provenance is immutable'); END;

CREATE TRIGGER owner_resource_provenance_retained
BEFORE DELETE ON owner_resource_provenance
BEGIN SELECT RAISE(ABORT, 'resource provenance must be retained'); END;
