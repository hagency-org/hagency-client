-- Ownership is recorded with the original reservation, before namespace IO.
-- Empty scope is confined to the original legacy profile store.
CREATE TABLE management_account_profiles (
 account_id TEXT PRIMARY KEY REFERENCES managed_accounts(id),
 profile TEXT NOT NULL CHECK(profile='' OR length(profile)=64)
) STRICT;
CREATE TRIGGER management_account_profiles_immutable BEFORE UPDATE ON management_account_profiles BEGIN SELECT RAISE(ABORT,'immutable account owner'); END;
CREATE TRIGGER management_account_profiles_retained BEFORE DELETE ON management_account_profiles BEGIN SELECT RAISE(ABORT,'retained account owner'); END;
