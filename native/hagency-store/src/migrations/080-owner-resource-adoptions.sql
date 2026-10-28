-- Definition-only transfer journal. No source is adopted by this migration.
CREATE TABLE owner_resource_adoptions (
 id TEXT PRIMARY KEY,
 profile TEXT NOT NULL UNIQUE CHECK(length(profile)=64),
 plan_digest TEXT NOT NULL UNIQUE CHECK(length(plan_digest)=64),
 plan TEXT NOT NULL,
 phase INTEGER NOT NULL CHECK(phase BETWEEN 0 AND 4),
 source_evidence TEXT,
 custody_evidence TEXT,
 result TEXT,
 CHECK((phase=0 AND source_evidence IS NULL) OR (phase>=1 AND source_evidence IS NOT NULL)),
 CHECK((phase<3 AND custody_evidence IS NULL) OR (phase>=3 AND custody_evidence IS NOT NULL)),
 CHECK((phase<4 AND result IS NULL) OR (phase=4 AND result IS NOT NULL))
) STRICT;
CREATE TRIGGER owner_resource_adoptions_identity BEFORE UPDATE ON owner_resource_adoptions
WHEN NEW.id!=OLD.id OR NEW.profile!=OLD.profile OR NEW.plan_digest!=OLD.plan_digest OR NEW.plan!=OLD.plan
 OR NEW.phase<OLD.phase OR NEW.phase>OLD.phase+1
 OR (OLD.source_evidence IS NOT NULL AND NEW.source_evidence IS NOT OLD.source_evidence)
 OR (OLD.custody_evidence IS NOT NULL AND NEW.custody_evidence IS NOT OLD.custody_evidence)
 OR (OLD.result IS NOT NULL AND NEW.result IS NOT OLD.result)
BEGIN SELECT RAISE(ABORT,'original resource adoption custody is immutable'); END;
CREATE TRIGGER owner_resource_adoptions_retained BEFORE DELETE ON owner_resource_adoptions
BEGIN SELECT RAISE(ABORT,'resource adoption custody must be retained'); END;
