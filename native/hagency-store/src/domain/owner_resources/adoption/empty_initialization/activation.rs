//! Empty initialization never imports a provider identity, seat or quota. The
//! original archive worker must retain and verify private command custody.
use super::*;
use crate::management::PrivateManagementJournal;

pub struct VerifiedEmptyDeviceJournal {
    authority: AdoptionAuthority,
    plan: EmptyPlan,
    journal: PrivateManagementJournal,
    evidence: String,
}
pub(crate) struct EmptyDeviceCustodyRequest {
    authority: AdoptionAuthority,
    plan: EmptyPlan,
    journal: PrivateManagementJournal,
}
impl EmptyDeviceCustodyRequest {
    pub(crate) fn check(&self) -> Result<(), Error> {
        self.authority.check()?;
        if !self.journal.writer_open() {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
    pub(crate) fn expires(&self) -> Instant {
        self.authority
            .expires
            .min(Instant::now() + Duration::from_secs(2))
    }
    pub(crate) fn execute(&self, db: &Connection) -> Result<VerifiedEmptyDeviceJournal, Error> {
        self.check()?;
        let (digest, count) = custody::collect(db, &[], true)?;
        if digest != self.plan.inventory.custody_digest
            || count != self.plan.inventory.completed_private_records
        {
            return Err(Error::Quarantined);
        }
        self.check()?;
        Ok(VerifiedEmptyDeviceJournal {
            authority: self.authority.clone(),
            plan: self.plan.clone(),
            journal: self.journal.clone(),
            evidence: custody_evidence(&self.plan)?,
        })
    }
}
impl VerifiedEmptyDeviceJournal {
    fn check(&self) -> Result<(), Error> {
        self.authority.check()?;
        if !self.journal.writer_open() || self.evidence != custody_evidence(&self.plan)? {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}
impl EmptyDeviceTransfer {
    /// Active restart validates permanent provenance, not the old mutable
    /// command snapshot. New account and task operations may already exist.
    #[cfg(feature = "resource-adoption")]
    pub async fn adopted_management(
        &self,
        journal: PrivateManagementJournal,
    ) -> Result<AdoptedManagementStore, Error> {
        AdoptedManagementStore::from_empty_device(
            &self.authority,
            &self.plan,
            journal,
            self.archive_binding()?,
        )
        .await
    }
    pub async fn verify_private_journal(
        &self,
        journal: &PrivateManagementJournal,
    ) -> Result<VerifiedEmptyDeviceJournal, Error> {
        self.authority.check()?;
        let result = journal
            .empty_device_custody(
                self.archive_binding()?,
                EmptyDeviceCustodyRequest {
                    authority: self.authority.clone(),
                    plan: self.plan.clone(),
                    journal: journal.clone(),
                },
            )
            .await?;
        self.authority.check()?;
        Ok(result)
    }
}
impl OwnerResourceRegistry {
    /// Validate the empty canonical namespace after the original sources are
    /// fenced. This phase creates no resource, account, seat or capacity grant.
    pub async fn prepare_empty_device_registry(
        &self,
        transfer: &EmptyDeviceTransfer,
    ) -> Result<EmptyDeviceInitializationSummary, Error> {
        transfer.authority.check()?;
        if !Arc::ptr_eq(&self.0, &transfer.authority.registry.0) {
            return Err(Error::LocalAuthority);
        }
        let authority = transfer.authority.clone();
        let original = transfer.plan.clone();
        let result = self
            .0
            .store
            .call(16384, move |db| {
                let tx = db
                    .db
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                let (plan, phase) = read_state(&tx, &authority, &original.initialization_id)?;
                if phase < 1 || plan.digest()? != original.digest()? {
                    return Err(Error::Conflict);
                }
                if phase < 4 {
                    ensure_profile_empty(&tx, &plan)?;
                }
                if phase == 1 {
                    tx.execute(
                        "UPDATE owner_resource_adoptions SET phase=2 WHERE id=?1",
                        [&plan.initialization_id],
                    )?;
                }
                authority.check()?;
                tx.commit()?;
                authority.check().map_err(|_| Error::OutcomeUnknown)?;
                plan.summary_at(phase.max(2))
            })
            .await?;
        transfer
            .authority
            .check()
            .map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
    pub async fn record_empty_device_custody(
        &self,
        proof: VerifiedEmptyDeviceJournal,
    ) -> Result<EmptyDeviceInitializationSummary, Error> {
        self.finish_empty_device(proof, false).await
    }
    pub async fn activate_empty_device(
        &self,
        proof: VerifiedEmptyDeviceJournal,
    ) -> Result<EmptyDeviceInitializationSummary, Error> {
        self.finish_empty_device(proof, true).await
    }
    async fn finish_empty_device(
        &self,
        proof: VerifiedEmptyDeviceJournal,
        activate: bool,
    ) -> Result<EmptyDeviceInitializationSummary, Error> {
        proof.check()?;
        if !Arc::ptr_eq(&self.0, &proof.authority.registry.0) {
            return Err(Error::LocalAuthority);
        }
        let after = proof.authority.clone();
        let journal = proof.journal.clone();
        let result = self.0.store.call(16384, move |db| {
            let tx = db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            proof.check()?;
            let (plan, phase) = read_state(&tx, &proof.authority, &proof.plan.initialization_id)?;
            if phase < if activate { 3 } else { 2 } || plan.digest()? != proof.plan.digest()? {
                return Err(Error::Conflict);
            }
            if phase < 4 { ensure_profile_empty(&tx, &plan)?; }
            let next = if activate && phase == 3 {
                tx.execute("UPDATE owner_resource_adoptions SET phase=4,result=?2 WHERE id=?1", params![plan.initialization_id,serde_json::to_string(&plan.summary_at(4)?)?])?;
                4
            } else if !activate && phase == 2 {
                tx.execute("UPDATE owner_resource_adoptions SET phase=3,custody_evidence=?2 WHERE id=?1", params![plan.initialization_id,proof.evidence])?;
                3
            } else { phase };
            proof.check()?;
            tx.commit()?;
            proof.check().map_err(|_| Error::OutcomeUnknown)?;
            plan.summary_at(next)
        }).await?;
        after.check().map_err(|_| Error::OutcomeUnknown)?;
        if !journal.writer_open() {
            return Err(Error::OutcomeUnknown);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
