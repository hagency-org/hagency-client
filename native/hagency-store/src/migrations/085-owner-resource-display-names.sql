-- Profile-local display metadata. Never part of effective resource, provider,
-- seat, grant, contribution, or execution authority. Empty names retain their
-- revision so clearing cannot make an old revision-zero request current again.
CREATE TABLE owner_resource_display_names (
    profile TEXT NOT NULL CHECK(length(profile)=64),
    resource_id TEXT NOT NULL REFERENCES resources(id),
    name TEXT NOT NULL CHECK(length(name)<=80 AND length(CAST(name AS BLOB))<=320),
    revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
    PRIMARY KEY(profile,resource_id)
);
