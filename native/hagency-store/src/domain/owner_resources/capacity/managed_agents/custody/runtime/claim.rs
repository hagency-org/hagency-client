use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedRuntimeHistory {
    pub command: ManagedRuntimeProvisionCommand,
    pub command_digest: String,
    pub issued_revision: Option<u64>,
    pub source_effect_state: String,
    pub permanently_stopped: bool,
    pub stop_revision: Option<u64>,
}
/// The sole first-issuance handle. No Clone, Deserialize, or history constructor.
/// A retained supervised adapter must consume this before any process spawn.
pub struct ManagedRuntimeCustody {
    pub(super) scope: Scope,
    pub(super) command: ManagedRuntimeProvisionCommand,
    pub(super) incarnation: String,
    pub(super) provider_digest: String,
    pub(super) enrollment: Arc<VerifiedManagementEnrollment>,
}
impl ManagedRuntimeCustody {
    /// Fixed at authenticated delivery; wall-clock changes cannot extend it.
    pub fn deadline(&self) -> Instant {
        self.scope.expires
    }
    pub fn effect_id(&self) -> &str {
        &self.scope.target
    }
    pub fn command(&self) -> &ManagedRuntimeProvisionCommand {
        &self.command
    }
    pub fn provider_digest(&self) -> &str {
        &self.provider_digest
    }
    pub fn incarnation(&self) -> &str {
        &self.incarnation
    }
}
pub enum ManagedRuntimeClaim {
    Issued(ManagedRuntimeCustody),
    History(ManagedRuntimeHistory),
}
fn history(
    db: &Connection,
    c: &ManagedRuntimeProvisionCommand,
) -> Result<Option<ManagedRuntimeHistory>, Error> {
    let row: Option<(String,String,Option<u64>,String)> = db.query_row("SELECT command_payload,command_digest,issued_revision,source_effect_state FROM owner_managed_runtime_lifetimes WHERE child_id=?1",[&c.agent_reservation_id],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let Some((payload, digest, issued, source_effect_state)) = row else {
        return Ok(None);
    };
    let command: ManagedRuntimeProvisionCommand = serde_json::from_str(&payload)?;
    if canonical::digest(&serde_json::to_value(&command)?)? != digest
        || digest != canonical::digest(&serde_json::to_value(c)?)?
    {
        return Err(Error::Conflict);
    }
    let stop: Option<u64> = db
        .query_row(
            "SELECT fence_revision FROM owner_managed_runtime_stops WHERE child_id=?1",
            [&c.agent_reservation_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(Some(ManagedRuntimeHistory {
        command,
        command_digest: digest,
        issued_revision: issued,
        source_effect_state,
        permanently_stopped: stop.is_some(),
        stop_revision: stop,
    }))
}
fn save(
    db: &Connection,
    p: &VerifiedManagedRuntimeDelivery,
    b: &Binding,
    binding: &str,
    issued: Option<u64>,
    incarnation: Option<&str>,
) -> Result<(), Error> {
    let c = p.command.original();
    db.execute(
        "INSERT INTO owner_managed_runtime_lifetimes VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            c.agent_reservation_id,
            p.enrollment.profile.digest,
            c.assignment_id,
            c.command_id,
            canonical::encode(&serde_json::to_value(c)?)?,
            canonical::digest(&serde_json::to_value(c)?)?,
            binding,
            b.provider,
            issued,
            incarnation,
            provision::history(db, &c.agent_reservation_id)?.state
        ],
    )?;
    Ok(())
}
impl OwnerResourceRegistry {
    /// Read-only preflight before separately supervised provider policy
    /// inspection. Returns no fresh proof for already-issued history; it never
    /// claims a process, changes a hold, or extends the original deadline.
    pub async fn prepare_managed_runtime_delivery(
        &self,
        p: VerifiedManagedRuntimeDelivery,
    ) -> Result<Option<VerifiedManagedRuntimeDelivery>, Error> {
        p.check()?;
        if !matches!(p.command, ManagedRuntimeCommand::Provision(_)) {
            return Err(Error::LocalAuthority);
        }
        let expires = p.deadline();
        let profile = p.enrollment.profile.clone();
        let registry = self.0.clone();
        let result = self
            .0
            .store
            .call(32 * 1024, move |db| {
                p.check()?;
                if registry.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                original(&db.db, &p)?;
                if history(&db.db, p.command.original())?.is_some() {
                    return Ok(None);
                }
                if provision::history(&db.db, &p.command.original().agent_reservation_id)?.state
                    != "pending"
                {
                    return Err(Error::OutcomeUnknown);
                }
                authority::original(
                    &db.db,
                    &p.enrollment,
                    &p.command.original().agent_reservation_id,
                    false,
                )?;
                if read_resource(&db.db, &p.command.original().reserve_command.resource_id)?
                    .framework
                    != "codex"
                {
                    return Err(Error::LocalAuthority);
                }
                p.check()?;
                Ok(Some(p))
            })
            .await?;
        after(&profile, &self.0, expires)?;
        Ok(result)
    }
    /// Atomically pins the immutable central command and marks existing effect
    /// issued. A returned history, lost reply or restarted process never reissues.
    pub async fn claim_managed_runtime(
        &self,
        p: VerifiedManagedRuntimeDelivery,
    ) -> Result<ManagedRuntimeClaim, Error> {
        p.check()?;
        if !matches!(p.command, ManagedRuntimeCommand::Provision(_)) {
            return Err(Error::LocalAuthority);
        }
        let profile = p.enrollment.profile.clone();
        let expires = p.deadline();
        let registry = self.0.clone();
        let result=self.0.store.call(32*1024,move |db| {
            p.check()?; if registry.closed.load(Ordering::Acquire) {return Err(Error::LocalAuthority);}
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (b,binding)=original(&tx,&p)?;
            let c=p.command.original();
            if let Some(h)=history(&tx,c)? { p.check()?; return Ok(ManagedRuntimeClaim::History(h)); }
            authority::original(&tx,&p.enrollment,&c.agent_reservation_id,false)?;
            if read_resource(&tx,&c.reserve_command.resource_id)?.framework != "codex" { return Err(Error::LocalAuthority); }
            let old=provision::history(&tx,&c.agent_reservation_id)?;
            if old.state!="pending" { return Err(Error::OutcomeUnknown); }
            tx.execute("INSERT OR IGNORE INTO owner_managed_assignment_pins VALUES(?1,?2,?3,?4,?5)",params![c.agent_reservation_id,c.assignment_id,c.assignment_generation,binding,b.provider])?;
            let revision=revision(&tx)?;
            let incarnation=format!("runtime_{revision}_{}",c.agent_reservation_id);
            save(&tx,&p,&b,&binding,Some(revision),Some(&incarnation))?;
            tx.execute("UPDATE owner_managed_effects SET state='issued',fence=?2 WHERE id=?1 AND state='pending'",params![old.effect_id,revision])?;
            let command=c.clone();
            let scope=Scope {registry:registry.clone(),profile:p.enrollment.profile.clone(),child:c.agent_reservation_id.clone(),target:old.effect_id,binding,issued_revision:revision,expires};
            p.check()?; tx.commit()?; p.check().map_err(|_|Error::OutcomeUnknown)?;
            Ok(ManagedRuntimeClaim::Issued(ManagedRuntimeCustody {scope,command,incarnation,provider_digest:b.provider,enrollment:Arc::new(p.enrollment)}))
        }).await?;
        after(&profile, &self.0, expires)?;
        Ok(result)
    }
    /// Records an irreversible stop obligation only. It neither claims that an
    /// issued process stopped nor refunds capacity. Absence is a tombstone.
    pub async fn fence_managed_runtime(
        &self,
        p: VerifiedManagedRuntimeDelivery,
    ) -> Result<ManagedRuntimeHistory, Error> {
        p.check()?;
        if !matches!(p.command, ManagedRuntimeCommand::Stop(_)) {
            return Err(Error::LocalAuthority);
        }
        let profile = p.enrollment.profile.clone();
        let expires = p.deadline();
        let registry = self.0.clone();
        let result=self.0.store.call(32*1024,move |db| {
            p.check()?;if registry.closed.load(Ordering::Acquire) {return Err(Error::LocalAuthority);}
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (b,binding)=original(&tx,&p)?;let c=p.command.original();
            if history(&tx,c)?.is_none() {save(&tx,&p,&b,&binding,None,None)?;}
            let old:Option<String>=tx.query_row("SELECT command_digest FROM owner_managed_runtime_stops WHERE child_id=?1",[&c.agent_reservation_id],|r|r.get(0)).optional()?;
            if let Some(old)=old {if old!=p.digest {return Err(Error::Conflict);}}
            else {
                let revision=revision(&tx)?;
                tx.execute("INSERT INTO owner_managed_runtime_stops VALUES(?1,?2,?3,?4,?5)",params![c.agent_reservation_id,p.command.id(),canonical::encode(&serde_json::to_value(&p.command)?)?,p.digest,revision])?;
                tx.execute("UPDATE owner_managed_agents SET state='fenced',permanent_fence=1 WHERE id=?1",[&c.agent_reservation_id])?;
                // Until the retained supervisor supplies evidence, issued and
                // never-issued cleanup are both conservative process unknown.
                tx.execute("UPDATE owner_managed_effects SET state='unknown',fence=?2 WHERE child_id=?1",params![c.agent_reservation_id,revision])?;
            }
            let h=history(&tx,c)?.ok_or(Error::Conflict)?;p.check()?;tx.commit()?;p.check().map_err(|_|Error::OutcomeUnknown)?;Ok(h)
        }).await?;
        after(&profile, &self.0, expires)?;
        Ok(result)
    }
    /// Read-only historical lookup. It cannot recreate issuance custody.
    pub async fn managed_runtime_history(
        &self,
        a: ManagedCleanupAuthority,
        command: ManagedRuntimeProvisionCommand,
    ) -> Result<Option<ManagedRuntimeHistory>, Error> {
        a.check(&self.0)?;
        command.validate(&a.enrollment.profile.account)?;
        if a.child != command.agent_reservation_id {
            return Err(Error::LocalAuthority);
        }
        let profile = a.enrollment.profile.clone();
        let expires = a.deadline();
        let registry = self.0.clone();
        let result = self
            .0
            .store
            .call(32 * 1024, move |db| {
                a.check(&registry)?;
                cleanup(&db.db, &a)?;
                let h = history(&db.db, &command)?;
                a.check(&registry)?;
                Ok(h)
            })
            .await?;
        after(&profile, &self.0, expires)?;
        Ok(result)
    }
}
