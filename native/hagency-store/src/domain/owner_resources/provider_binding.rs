//! Read-only binding from a real provider observation to canonical owner state.
use super::*;

/// Native-only, non-deserializable observation. It cannot enroll or allocate a
/// resource and carries no raw provider subject or credential.
pub struct VerifiedOwnerResourceBinding {
    pub(super) registry_id: String,
    pub(super) profile: ProfileAuthority,
    pub(super) resource_id: String,
    pub(super) resource_revision: String,
    runtime: String,
    seat_id: String,
    provider_digest: String,
    access_revision: u64,
    pub(super) expires: Instant,
}
impl VerifiedOwnerResourceBinding {
    pub fn resource_id(&self) -> &str {
        &self.resource_id
    }
    pub fn resource_revision(&self) -> &str {
        &self.resource_revision
    }
    pub fn runtime(&self) -> &str {
        &self.runtime
    }
    /// A private journal commitment, never a serialized authority proof.
    pub fn binding_digest(&self) -> Result<String, Error> {
        self.check()?;
        canonical::digest(&json!({
            "registry": self.registry_id, "profile": self.profile.digest,
            "owner": self.profile.account, "resource": self.resource_id,
            "revision": self.resource_revision, "runtime": self.runtime,
            "seat": self.seat_id, "provider": self.provider_digest,
            "accessRevision": self.access_revision,
        }))
        .map_err(Into::into)
    }
    pub(super) fn check(&self) -> Result<(), Error> {
        self.profile.check()?;
        if Instant::now() >= self.expires {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
    pub(super) fn check_canonical(&self, db: &Connection, registry: &str) -> Result<(), Error> {
        self.check()?;
        if registry != self.registry_id {
            return Err(Error::LocalAuthority);
        }
        authorize_resource(db, &self.profile, &self.resource_id)?;
        let resource = read_resource(db, &self.resource_id)?;
        let access: u64 = db.query_row(
            "SELECT revision FROM owner_resource_grants WHERE profile=?1 AND resource_id=?2 AND active=1",
            params![self.profile.digest,self.resource_id], |r| r.get(0),
        )?;
        let account: Option<(String,String)> = db.query_row(
            "SELECT provider_digest,identity_kind FROM owner_resource_accounts WHERE seat_id=?1",
            [&resource.seat_id], |r| Ok((r.get(0)?,r.get(1)?)),
        ).optional()?;
        if access != self.access_revision
            || resource.seat_id != self.seat_id
            || resource.framework != self.runtime
            || crate::resource_publication_revision(&resource)? != self.resource_revision
            || account != Some((self.provider_digest.clone(), "verified_provider".into()))
        {
            return Err(Error::LocalAuthority);
        }
        self.check()
    }
}
impl OwnerResourceRegistry {
    pub async fn verify_provider_resource(
        &self,
        profile: ProfileAuthority,
        resource_id: String,
        provider: VerifiedProviderAccount,
    ) -> Result<VerifiedOwnerResourceBinding, Error> {
        profile.check()?;
        hagency_core::project::identifier(&resource_id, 128)?;
        if Instant::now() >= provider.expires || self.0.closed.load(Ordering::Acquire) {
            return Err(Error::LocalAuthority);
        }
        let fence = self.0.clone();
        let caller = profile.clone();
        let result = self.0.store.call(4096, move |db| {
            profile.check()?;
            if fence.closed.load(Ordering::Acquire) || Instant::now() >= provider.expires {
                return Err(Error::LocalAuthority);
            }
            authorize_resource(&db.db,&profile,&resource_id)?;
            let resource = read_resource(&db.db,&resource_id)?;
            let access_revision = db.db.query_row(
                "SELECT revision FROM owner_resource_grants WHERE profile=?1 AND resource_id=?2 AND active=1",
                params![profile.digest,resource_id], |r| r.get(0),
            )?;
            let result = VerifiedOwnerResourceBinding {
                registry_id: fence.id.clone(), profile, resource_id,
                resource_revision: crate::resource_publication_revision(&resource)?,
                runtime: provider.runtime, seat_id: resource.seat_id,
                provider_digest: provider.digest, access_revision, expires: provider.expires,
            };
            result.check_canonical(&db.db,&fence.id)?;
            if fence.closed.load(Ordering::Acquire) { return Err(Error::LocalAuthority); }
            Ok(result)
        }).await?;
        caller.check()?;
        result.check()?;
        if self.0.closed.load(Ordering::Acquire) {
            return Err(Error::LocalAuthority);
        }
        Ok(result)
    }
}
