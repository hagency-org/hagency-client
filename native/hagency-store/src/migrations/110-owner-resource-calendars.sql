-- Schema108. The owner-approved calendar is separate from the immutable
-- offered resource definition. All later grants use this same calendar.
CREATE TABLE IF NOT EXISTS owner_resource_calendars (
 resource_id TEXT PRIMARY KEY REFERENCES resources(id),
 profile TEXT NOT NULL CHECK(length(profile)=64),
 calendar TEXT NOT NULL CHECK(json_valid(calendar) AND length(calendar)<512),
 source_grant_id TEXT NOT NULL REFERENCES owner_capacity_grants(id) DEFERRABLE INITIALLY DEFERRED,
 created_at_ms INTEGER NOT NULL CHECK(created_at_ms>0 AND created_at_ms<=9007199254740991)
) STRICT;
CREATE TRIGGER IF NOT EXISTS owner_resource_calendar_immutable BEFORE UPDATE ON owner_resource_calendars
BEGIN SELECT RAISE(ABORT,'a published resource retains its calendar'); END;
CREATE TRIGGER IF NOT EXISTS owner_resource_calendar_retained BEFORE DELETE ON owner_resource_calendars
BEGIN SELECT RAISE(ABORT,'resource calendar history is retained'); END;
