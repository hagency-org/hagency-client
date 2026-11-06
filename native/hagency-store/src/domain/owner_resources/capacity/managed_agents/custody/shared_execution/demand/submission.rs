//! Original request submission is a separate lane from manual Start recovery.
use super::*;

/// A finite original demand request, still not a process/activity authority.
/// The eventual B adapter must check it immediately before its authenticated
/// POST; the server separately atomically checks current Run/Stop revision.
pub struct SharedDemandSubmission {
    registry: Arc<Inner>,
    proof: VerifiedSharedDemand,
    original: SharedRuntimeCommand,
    input: SharedDemandStartInput,
}
impl SharedDemandSubmission {
    pub(super) fn new(
        registry: Arc<Inner>,
        proof: VerifiedSharedDemand,
        original: SharedRuntimeCommand,
        input: SharedDemandStartInput,
    ) -> Self {
        Self {
            registry,
            proof,
            original,
            input,
        }
    }
    pub fn check(&self) -> Result<(), Error> {
        self.proof.check()?;
        after(
            &self.proof.enrollment.profile,
            &self.registry,
            self.proof.expires,
        )
    }
    pub fn input(&self) -> &SharedDemandStartInput {
        &self.input
    }
    pub fn matches_command(&self, command: &SharedAttemptCommand) -> Result<(), Error> {
        self.check()?;
        command.validate(&self.proof.enrollment.profile.account)?;
        SharedAttemptStartIntent {
            input: self.input.start.clone(),
            original: self.original.clone(),
        }
        .matches_command(command)
    }
}

/// Descriptive restart cursor, with no current effect permission.
pub struct SharedDemandRecovery {
    original: SharedRuntimeCommand,
    intent: SharedDemandIntent,
}
impl SharedDemandRecovery {
    pub fn original(&self) -> &SharedRuntimeCommand {
        &self.original
    }
    pub fn intent(&self) -> &SharedDemandIntent {
        &self.intent
    }
}

impl OwnerResourceRegistry {
    /// Recover at most one retained entry. The caller rotates the original ID
    /// cursor; this operation never chooses an ID, rebases a head or sends IO.
    pub async fn next_shared_demand(
        &self,
        profile: ProfileAuthority,
        device: String,
        after_id: Option<String>,
    ) -> Result<Option<SharedDemandRecovery>, Error> {
        self.next_saved_demand(profile, device, after_id, false).await
    }

    /// The next saved demand that is not finished locally: no binding, no
    /// terminal and no blocked decision. Only these can hold an Agent's FIFO,
    /// so a separate pass over them releases an interrupted demand within a few
    /// worker passes after a restart; the full walk above still revisits bound
    /// demands, whose central state can still turn cancelled.
    pub async fn next_unfinished_shared_demand(
        &self,
        profile: ProfileAuthority,
        device: String,
        after_id: Option<String>,
    ) -> Result<Option<SharedDemandRecovery>, Error> {
        self.next_saved_demand(profile, device, after_id, true).await
    }

    async fn next_saved_demand(
        &self,
        profile: ProfileAuthority,
        device: String,
        after_id: Option<String>,
        unfinished_only: bool,
    ) -> Result<Option<SharedDemandRecovery>, Error> {
        profile.check()?;
        identifier(&device, 128)?;
        if let Some(id) = &after_id {
            identifier(id, 128)?;
        }
        let p = profile.clone();
        let expires = p.expires;
        let registry = self.0.clone();
        let result=self.0.store.call(64*1024,move |db| {
            after(&profile,&registry,expires)?;
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let row:Option<(String,String,String,String)>=tx.query_row(
                "SELECT d.id,d.child_id,l.command_payload,l.command_digest FROM owner_shared_demands d JOIN owner_shared_runtime_lifetimes l ON l.child_id=d.child_id AND l.profile=d.profile WHERE d.profile=?1 AND d.device_id=?2 AND (?3 IS NULL OR d.id>?3) AND (?4=0 OR (NOT EXISTS(SELECT 1 FROM owner_shared_demand_bindings b WHERE b.demand_id=d.id) AND NOT EXISTS(SELECT 1 FROM owner_shared_budget_demand_terminals t WHERE t.demand_id=d.id) AND NOT EXISTS(SELECT 1 FROM owner_shared_demand_decisions x WHERE x.demand_id=d.id AND x.disposition IN('budget_blocked','head_changed')))) ORDER BY d.id LIMIT 1",
                params![profile.digest,device,after_id,unfinished_only as i64],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
            ).optional()?;
            let result=row.map(|(id,child,payload,digest)|->Result<SharedDemandRecovery,Error>{
                let original:SharedRuntimeCommand=serde_json::from_str(&payload)?;
                original.validate(&profile.account)?;
                if original.agent_reservation_id!=child||original.reserve_command.executor_device_id!=device||canonical::digest(&serde_json::to_value(&original)?)?!=digest{return Err(Error::Conflict)}
                let intent=journal::load(&tx,&profile.digest,&child,&id)?.ok_or(Error::Conflict)?;
                if intent.offer.assignment_id!=original.assignment_id{return Err(Error::Conflict)}
                Ok(SharedDemandRecovery {original,intent})
            }).transpose()?;
            after(&profile,&registry,expires)?;tx.commit()?;Ok(result)
        }).await?;
        after(&p, &self.0, expires)?;
        Ok(result)
    }

    /// Revalidate a saved original request without creating or rewriting it.
    /// Missing/blocked/changed custody cannot be converted to a fresh Start.
    pub async fn shared_demand_submission(
        &self,
        proof: VerifiedSharedDemand,
        original: SharedRuntimeCommand,
    ) -> Result<SharedDemandSubmission, Error> {
        if !cfg!(feature = "shared-intake") {
            return Err(Error::Unqualified);
        }
        proof.check()?;
        original.validate(&proof.enrollment.profile.account)?;
        let p = proof.enrollment.profile.clone();
        let expires = proof.expires;
        let registry = self.0.clone();
        let result = self
            .0
            .store
            .call(64 * 1024, move |db| {
                proof.check()?;
                after(&p, &registry, expires)?;
                let tx = db
                    .db
                    .transaction_with_behavior(TransactionBehavior::Deferred)?;
                let child = &original.agent_reservation_id;
                let b = authority::original(&tx, &proof.enrollment, child, false)?;
                if b.digest != original.reserve_command_digest
                    || original.reserve_command.executor_device_id
                        != proof.enrollment.executor_device_id
                    || original.reserve_command.executor_generation
                        != proof.enrollment.executor_generation
                {
                    return Err(Error::LocalAuthority);
                }
                let saved = journal::load(&tx, &p.digest, child, &proof.offer.demand_id)?
                    .ok_or(Error::LocalAuthority)?;
                if saved.offer.digest()? != proof.offer.digest()? {
                    return Err(Error::Conflict);
                }
                journal::control(&tx, &p.digest, child, proof.offer.control_revision)?;
                if tx.query_row("SELECT EXISTS(SELECT 1 FROM owner_shared_budget_demand_terminals WHERE demand_id=?1)", [&proof.offer.demand_id], |r| r.get::<_, bool>(0))? {
                    return Err(Error::LocalAuthority);
                }
                let input = saved.input.ok_or(Error::Capacity)?;
                let start = start_selection::saved_start(&tx, &p, &input.start.start_intent_id)?;
                if canonical::digest(&serde_json::to_value(start.original())?)?
                    != canonical::digest(&serde_json::to_value(&original)?)?
                {
                    return Err(Error::Conflict);
                }
                proof.check()?;
                tx.commit()?;
                Ok(SharedDemandSubmission {
                    registry,
                    proof,
                    original,
                    input,
                })
            })
            .await?;
        after(&result.proof.enrollment.profile, &self.0, expires)?;
        Ok(result)
    }
}
