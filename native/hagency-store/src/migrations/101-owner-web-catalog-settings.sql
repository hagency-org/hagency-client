-- Owner-local web-console settings, never server approval authority.
-- Kinds are explicitly selected by each typed native adapter. There is no KV API.
CREATE TABLE owner_web_catalog_settings (
 profile TEXT NOT NULL CHECK(length(profile)=64),
 kind TEXT NOT NULL CHECK(length(kind) BETWEEN 1 AND 64),
 entity TEXT NOT NULL CHECK(length(entity) BETWEEN 1 AND 2048),
 value TEXT NOT NULL CHECK(length(value)<=65536 AND json_valid(value)),
 PRIMARY KEY(profile,kind,entity)
);
