use super::*;
use crate::management::{ArchiveJournalBinding, PrivateManagementJournal};
/// Successful validation on the original journal's retained worker. It grants
/// no private-row access and cannot be constructed from a serialized receipt.
pub struct VerifiedAdoptionJournal {
    pub(super) authority: AdoptionAuthority,
    pub(super) plan: Plan,
    pub(super) evidence: String,
}
pub(crate) struct CustodyRequest {
    authority: AdoptionAuthority,
    plan: Plan,
}
impl CustodyRequest {
    pub(crate) fn check(&self) -> Result<(), Error> {
        self.authority.check()
    }
    pub(crate) fn expires(&self) -> Instant {
        self.authority
            .expires
            .min(Instant::now() + Duration::from_secs(2))
    }
    pub(crate) fn execute(&self, db: &Connection) -> Result<VerifiedAdoptionJournal, Error> {
        self.check()?;
        let (digest, count) = custody::collect(db, &self.plan.inventory.resources, self.plan.version == 3)?;
        if digest != self.plan.inventory.custody_digest || count != self.plan.inventory.custody_rows
        {
            return Err(Error::Quarantined);
        }
        self.check()?;
        Ok(VerifiedAdoptionJournal {
            authority: self.authority.clone(),
            plan: self.plan.clone(),
            evidence: canonical::digest(
                &json!({"plan":self.plan.digest()?,"custody":digest,"count":count}),
            )?,
        })
    }
}
impl VerifiedAdoptionJournal {
    pub(super) fn check(&self, current: &AdoptionAuthority) -> Result<(), Error> {
        self.authority.check()?;
        current.check()?;
        if !Arc::ptr_eq(&self.authority.registry.0, &current.registry.0)
            || self.authority.profile.digest != current.profile.digest
            || self.authority.profile.account != current.profile.account
        {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}
impl AdoptionTransfer {
    pub fn archive_binding(&self) -> Result<ArchiveJournalBinding, Error> {
        self.authority.check()?;
        ArchiveJournalBinding::new(
            self.plan.migration_id.clone(),
            self.plan.registry_id.clone(),
            self.plan.inventory.profile.clone(),
            self.plan.inventory.owner.clone(),
            self.plan.inventory_digest.clone(),
            self.plan.inventory.source_schema,
        )
    }
    pub async fn verify_private_journal(
        &self,
        journal: &PrivateManagementJournal,
    ) -> Result<VerifiedAdoptionJournal, Error> {
        self.authority.check()?;
        let result = journal
            .adoption_custody(
                self.archive_binding()?,
                CustodyRequest {
                    authority: self.authority.clone(),
                    plan: self.plan.clone(),
                },
            )
            .await?;
        self.authority.check()?;
        Ok(result)
    }
}
impl OwnerResourceRegistry {
    pub async fn record_adoption_custody(
        &self,
        authority: AdoptionAuthority,
        proof: VerifiedAdoptionJournal,
    ) -> Result<AdoptionSummary, Error> {
        proof.check(&authority)?;
        if !Arc::ptr_eq(&self.0, &authority.registry.0) {
            return Err(Error::LocalAuthority);
        }
        let after = authority.clone();
        let result = self
            .0
            .store
            .call(4096, move |db| db.finish_adoption(&authority, &proof, None))
            .await?;
        after.check().map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
    pub async fn activate_resource_adoption(
        &self,
        authority: AdoptionAuthority,
        proof: VerifiedAdoptionJournal,
        provider: VerifiedProviderAccount,
    ) -> Result<AdoptionSummary, Error> {
        proof.check(&authority)?;
        if !Arc::ptr_eq(&self.0, &authority.registry.0) {
            return Err(Error::LocalAuthority);
        }
        let after = authority.clone();
        let result = self
            .0
            .store
            .call(4096, move |db| {
                db.finish_adoption(&authority, &proof, Some(&provider))
            })
            .await?;
        after.check().map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
}
