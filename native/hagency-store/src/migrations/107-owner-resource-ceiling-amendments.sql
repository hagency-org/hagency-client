-- Schema104: the monthly maximum of a resource that was offered, published or
-- retains commitments changes through append-only native amendments. Its
-- definition, revision and provenance stay byte-identical, so server state,
-- grant receipts, reserve commands and Agent custody are never invalidated.
-- The latest amendment is the effective maximum; absence keeps the definition's.
CREATE TABLE owner_resource_ceiling_amendments (
 resource_id TEXT NOT NULL REFERENCES resources(id),
 sequence INTEGER NOT NULL CHECK(sequence>0 AND sequence<=9007199254740991),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 command_id TEXT NOT NULL UNIQUE CHECK(length(command_id) BETWEEN 1 AND 128),
 base_revision TEXT NOT NULL CHECK(length(base_revision)=64),
 previous_tokens INTEGER NOT NULL CHECK(previous_tokens>=0 AND previous_tokens<=9007199254740991),
 tokens INTEGER NOT NULL CHECK(tokens>0 AND tokens<=9007199254740991 AND tokens<>previous_tokens),
 period TEXT NOT NULL CHECK(period='monthly'),
 created_at_ms INTEGER NOT NULL CHECK(created_at_ms>=0 AND created_at_ms<=9007199254740991),
 PRIMARY KEY(resource_id,sequence)
) STRICT;

-- Each amendment continues the one before it, the first one the definition's
-- own monthly maximum. A gap, a fork or an unrelated starting value is refused.
CREATE TRIGGER owner_resource_ceiling_amendments_sequential
BEFORE INSERT ON owner_resource_ceiling_amendments
WHEN NEW.sequence IS NOT 1+COALESCE((SELECT max(sequence) FROM owner_resource_ceiling_amendments WHERE resource_id=NEW.resource_id),0)
 OR NEW.previous_tokens IS NOT COALESCE(
   (SELECT tokens FROM owner_resource_ceiling_amendments WHERE resource_id=NEW.resource_id AND sequence=NEW.sequence-1),
   (SELECT json_extract(config,'$.ceiling.tokens') FROM resources WHERE id=NEW.resource_id AND json_extract(config,'$.ceiling.period')='monthly'))
BEGIN SELECT RAISE(ABORT, 'resource ceiling amendments continue the effective monthly maximum'); END;

CREATE TRIGGER owner_resource_ceiling_amendments_immutable
BEFORE UPDATE ON owner_resource_ceiling_amendments
BEGIN SELECT RAISE(ABORT, 'resource ceiling amendments are immutable'); END;

CREATE TRIGGER owner_resource_ceiling_amendments_retained
BEFORE DELETE ON owner_resource_ceiling_amendments
BEGIN SELECT RAISE(ABORT, 'resource ceiling amendments must be retained'); END;

-- Once amended, the definition keeps the maximum its first amendment started
-- from. Otherwise a later definition write could silently diverge from the
-- amendment chain that every capacity reader treats as authoritative.
CREATE TRIGGER owner_resource_amended_ceiling_retained
BEFORE UPDATE OF config ON resources
WHEN EXISTS(SELECT 1 FROM owner_resource_ceiling_amendments WHERE resource_id=OLD.id)
 AND json_extract(OLD.config,'$.ceiling') IS NOT json_extract(NEW.config,'$.ceiling')
BEGIN SELECT RAISE(ABORT, 'an amended resource keeps its original monthly maximum'); END;
