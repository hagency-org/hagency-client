-- Schema 74; filenames 074/075 belong to older board-numbered migrations.
-- Existing rows deliberately remain unknown: schema migration is not evidence
-- that an old hold had no execution or external liabilities.
ALTER TABLE owner_capacity_grants ADD COLUMN accounting_mode TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE owner_project_reservations ADD COLUMN accounting_mode TEXT NOT NULL DEFAULT 'unknown';
CREATE TABLE owner_parent_settlements (
    grant_id TEXT PRIMARY KEY REFERENCES owner_capacity_grants(id),
    fence_revision INTEGER NOT NULL CHECK(fence_revision > 0 AND fence_revision <= 9007199254740991),
    retained_tokens INTEGER NOT NULL CHECK(retained_tokens >= 0 AND retained_tokens <= 9007199254740991),
    receipt TEXT NOT NULL CHECK(json_valid(receipt)),
    receipt_digest TEXT NOT NULL CHECK(length(receipt_digest) = 64),
    manifest TEXT NOT NULL CHECK(json_valid(manifest))
) STRICT;
