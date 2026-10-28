use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedProvisionHistory {
    pub effect_id: String,
    pub state: String,
    pub fence_revision: u64,
    pub result_digest: Option<String>,
}
pub struct ManagedProvisionCustody {
    pub(super) scope: Scope,
}
pub enum ManagedProvisionClaim {
    Issued(ManagedProvisionCustody),
    History(ManagedProvisionHistory),
}
impl ManagedProvisionCustody {
    pub fn effect_id(&self) -> &str {
        &self.scope.target
    }
}
pub(super) fn history(db: &Connection, child: &str) -> Result<ManagedProvisionHistory, Error> {
    Ok(db.query_row(
        "SELECT id,state,fence,original_result_digest FROM owner_managed_effects WHERE child_id=?1",
        [child],
        |r| {
            Ok(ManagedProvisionHistory {
                effect_id: r.get(0)?,
                state: r.get(1)?,
                fence_revision: r.get(2)?,
                result_digest: r.get(3)?,
            })
        },
    )?)
}
impl OwnerResourceRegistry {
    /// Only first issuance yields custody. A historical `issued` result after
    /// process loss is uncertainty, not an invitation to initialize again.
    pub async fn claim_managed_provision(
        &self,
        a: ManagedAssignmentAuthority,
    ) -> Result<ManagedProvisionClaim, Error> {
        a.check(&self.0)?;
        let profile = a.enrollment.profile.clone();
        let expires = a.deadline();
        let registry = self.0.clone();
        let result=self.0.store.call(8192,move |db| {
            a.check(&registry)?;
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (_,binding)=assignment(&tx,&a,true)?;
            let h=history(&tx,&a.child)?;
            if h.state!="pending" {a.check(&registry)?;return Ok(ManagedProvisionClaim::History(h));}
            let revision=revision(&tx)?;
            tx.execute("UPDATE owner_managed_effects SET state='issued',fence=?2 WHERE id=?1 AND state='pending'",params![h.effect_id,revision])?;
            let custody=ManagedProvisionCustody {scope:Scope {registry:registry.clone(),profile:a.enrollment.profile.clone(),child:a.child.clone(),target:h.effect_id,binding,issued_revision:revision,expires}};
            a.check(&registry)?;tx.commit()?;
            a.check(&registry).map_err(|_|Error::OutcomeUnknown)?;
            Ok(ManagedProvisionClaim::Issued(custody))
        }).await?;
        after(&profile, &self.0, expires)?;
        Ok(result)
    }
    pub async fn managed_provision_history(
        &self,
        a: ManagedCleanupAuthority,
    ) -> Result<ManagedProvisionHistory, Error> {
        a.check(&self.0)?;
        let p = a.enrollment.profile.clone();
        let expires = a.deadline();
        let r = self.0.clone();
        let result = self
            .0
            .store
            .call(8192, move |db| {
                a.check(&r)?;
                cleanup(&db.db, &a)?;
                let result = history(&db.db, &a.child)?;
                a.check(&r)?;
                Ok(result)
            })
            .await?;
        after(&p, &self.0, expires)?;
        Ok(result)
    }
    pub async fn record_managed_provision(
        &self,
        c: &ManagedProvisionCustody,
        o: ManagedProvisionObservation,
    ) -> Result<ManagedObservationReceipt, Error> {
        c.scope.check(&self.0)?;
        let s = c.scope.clone();
        let p = s.profile.clone();
        let expires = s.expires;
        let registry = self.0.clone();
        let result = self
            .0
            .store
            .call(16 * 1024, move |db| {
                s.check(&registry)?;
                let tx = db
                    .db
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                observations::check_scope(&tx, &s, "provision")?;
                let result = observations::provision(&tx, &s.child, &s.target, o)?;
                s.check(&registry)?;
                tx.commit()?;
                s.check(&registry).map_err(|_| Error::OutcomeUnknown)?;
                Ok(result)
            })
            .await?;
        after(&p, &self.0, expires)?;
        Ok(result)
    }
    pub async fn record_late_managed_provision(
        &self,
        a: ManagedCleanupAuthority,
        effect: String,
        o: ManagedProvisionObservation,
    ) -> Result<ManagedObservationReceipt, Error> {
        a.check(&self.0)?;
        identifier(&effect, 160)?;
        let p = a.enrollment.profile.clone();
        let expires = a.deadline();
        let r = self.0.clone();
        let result = self
            .0
            .store
            .call(16 * 1024, move |db| {
                a.check(&r)?;
                let tx = db
                    .db
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                cleanup(&tx, &a)?;
                let result = observations::provision(&tx, &a.child, &effect, o)?;
                a.check(&r)?;
                tx.commit()?;
                a.check(&r).map_err(|_| Error::OutcomeUnknown)?;
                Ok(result)
            })
            .await?;
        after(&p, &self.0, expires)?;
        Ok(result)
    }
}
