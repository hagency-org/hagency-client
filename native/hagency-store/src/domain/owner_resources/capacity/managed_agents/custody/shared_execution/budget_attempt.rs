//! V3 one-shot process custody. Old command/receipt bytes remain unchanged.
use super::*;

pub(super) fn control_revision(db: &Connection, profile: &str, c: &SharedBudgetAttemptCommand) -> Result<Option<u64>, Error> {
    budget_demand::control_revision(db, profile, c)
}
/// Fresh authenticated native delivery; history cannot construct it.
pub struct VerifiedSharedBudgetAttemptDelivery {
    enrollment: VerifiedManagementEnrollment,
    command: SharedBudgetAttemptCommand,
    digest: String,
    expires: Instant,
}
impl VerifiedSharedBudgetAttemptDelivery {
    pub fn new(
        enrollment: VerifiedManagementEnrollment,
        command: SharedBudgetAttemptCommand,
        digest: String,
        execution_allowed: bool,
        expires: Instant,
    ) -> Result<Self, Error> {
        enrollment.check()?;
        command.validate(&enrollment.profile.account)?;
        if !execution_allowed
            || expires > Instant::now() + Duration::from_secs(30)
            || digest != canonical::digest(&serde_json::to_value(&command)?)?
        {
            return Err(Error::LocalAuthority);
        }
        let expires = expires.min(enrollment.expires).min(
            Instant::now()
                + Duration::from_millis(command.expires_at_ms.saturating_sub(graphs::now_ms()?)),
        );
        let result = Self {
            enrollment,
            command,
            digest,
            expires,
        };
        result.check()?;
        Ok(result)
    }
    fn check(&self) -> Result<(), Error> {
        self.enrollment.check()?;
        let now = graphs::now_ms()?;
        if Instant::now() >= self.expires
            || now < self.command.issued_at_ms
            || now >= self.command.expires_at_ms
            || self.enrollment.executor_generation
                != self
                    .command
                    .original_runtime_command
                    .reserve_command
                    .executor_generation
        {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetAttemptHistory {
    pub command: SharedBudgetAttemptCommand,
    pub command_digest: String,
    pub issued_revision: u64,
}
/// Exactly one process-attempt claim. No Clone/Deserialize; actual-process
/// binding records evidence and does not create model-call authority.
pub struct SharedBudgetAttemptCustody {
    pub(super) registry: Arc<Inner>,
    pub(super) enrollment: VerifiedManagementEnrollment,
    pub(super) command: SharedBudgetAttemptCommand,
    pub(super) expires: Instant,
    pub(super) incarnation: String,
    pub(super) digest: String,
    pub(super) provider: String,
    pub(super) issued_revision: u64,
    control_revision: Option<u64>,
}
impl SharedBudgetAttemptCustody {
    /// Exact demand Run pin from the claim; descriptive cancellation scope only.
    pub fn demand_control_revision(&self) -> Option<u64> {
        self.control_revision
    }

    pub fn command(&self) -> &SharedBudgetAttemptCommand {
        &self.command
    }
    pub fn incarnation(&self) -> &str {
        &self.incarnation
    }
    pub fn check(&self) -> Result<(), Error> {
        self.enrollment.check()?;
        after(&self.enrollment.profile, &self.registry, self.expires)
    }
}
pub enum SharedBudgetAttemptClaim {
    Issued(Box<SharedBudgetAttemptCustody>),
    History(Box<SharedBudgetAttemptHistory>),
}

fn history(
    db: &Connection,
    profile: &str,
    c: &SharedBudgetAttemptCommand,
) -> Result<Option<SharedBudgetAttemptHistory>, Error> {
    let row:Option<(String,String,String,u64)>=db.query_row("SELECT profile,command_payload,command_digest,issued_revision FROM owner_shared_successor_attempts WHERE id=?1",[&c.command_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let Some((saved_profile, payload, digest, issued_revision)) = row else {
        return Ok(None);
    };
    let saved: SharedBudgetAttemptCommand = serde_json::from_str(&payload)?;
    if saved_profile != profile
        || canonical::digest(&serde_json::to_value(&saved)?)? != digest
        || canonical::digest(&serde_json::to_value(c)?)? != digest
    {
        return Err(Error::Conflict);
    }
    Ok(Some(SharedBudgetAttemptHistory {
        command: saved,
        command_digest: digest,
        issued_revision,
    }))
}

impl OwnerResourceRegistry {
    /// Historical command custody under fresh same-installation authorization.
    /// No process right is reconstructed, even after a generation change.
    pub async fn shared_budget_successor_history(
        &self,
        authority: ManagedCleanupAuthority,
        command: SharedBudgetAttemptCommand,
    ) -> Result<Option<SharedBudgetAttemptHistory>, Error> {
        authority.check(&self.0)?;
        command.validate(&authority.enrollment.profile.account)?;
        if authority.child != command.agent_reservation_id {
            return Err(Error::LocalAuthority);
        }
        let profile = authority.enrollment.profile.clone();
        let expires = authority.deadline();
        let registry = self.0.clone();
        let result = self
            .0
            .store
            .call(64 * 1024, move |db| {
                authority.check(&registry)?;
                authority::cleanup(&db.db, &authority)?;
                let value = history(&db.db, &authority.enrollment.profile.digest, &command)?;
                authority.check(&registry)?;
                Ok(value)
            })
            .await?;
        after(&profile, &self.0, expires)?;
        Ok(result)
    }

    pub async fn claim_shared_budget_successor(
        &self,
        p: VerifiedSharedBudgetAttemptDelivery,
    ) -> Result<SharedBudgetAttemptClaim, Error> {
        p.check()?;
        let profile = p.enrollment.profile.clone();
        let expires = p.expires;
        let registry = self.0.clone();
        let value=self.0.store.call(64*1024,move|db|{
            p.check()?;after(&p.enrollment.profile,&registry,expires)?;
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let c=&p.command;
            authority::original(&tx,&p.enrollment,&c.agent_reservation_id,true)?;
            if let Some(old)=history(&tx,&p.enrollment.profile.digest,c)?{p.check()?;after(&p.enrollment.profile,&registry,expires)?;return Ok(SharedBudgetAttemptClaim::History(Box::new(old)))}
            budget_fence::require_unfenced(&tx,c)?;
            if owner_tool_policy(&tx,&c.original_runtime_command)? != c.tool_policy {return Err(Error::LocalAuthority)}
            let control_revision=budget_attempt::control_revision(&tx, &p.enrollment.profile.digest, c)?;
            let now=graphs::now_ms()?;
            let b=authority::original(&tx,&p.enrollment,&c.agent_reservation_id,false)?;
            if now>=authority::lifetime(&tx,&b,now)?{return Err(Error::LocalAuthority)}
            let selection=budget_snapshot::snapshot(&tx,&p.enrollment,&c.original_runtime_command)?;
            if selection.execution_budget != c.execution_budget
                || canonical::digest(&serde_json::to_value(&selection.predecessor)?)? != canonical::digest(&serde_json::to_value(&c.predecessor)?)? {return Err(Error::Conflict)}
            let current=selection.budget;
            attempt_work::account_budget(&tx,&c.agent_reservation_id,&b)?;
            let expected=&c.budget_observation;
            if current.state!=SharedSuccessorBudgetState::Available
                || current.observed_ledger_revision<expected.observed_ledger_revision
                || current.allocated_tokens!=expected.allocated_tokens
                || current.known_consumed_tokens!=expected.known_consumed_tokens
                || current.retained_tokens!=expected.retained_tokens
                || current.remaining_tokens!=expected.remaining_tokens
                || current.usage_complete!=expected.usage_complete
                || current.liability_digest!=expected.liability_digest
                || current.predecessor_command_digest!=c.predecessor.command_digest
                || current.stopped_receipt_digest!=c.predecessor.stopped_receipt_digest {
                return Err(Error::Conflict)
            }
            let process:String=if c.predecessor.process_digest.is_empty() {
                // snapshot already verified the immutable not_started head.
                String::new()
            }else if c.process_attempt==2 {
                tx.query_row("SELECT json_extract(payload,'$.processDigest') FROM owner_shared_process_observations WHERE child_id=?1 AND digest=?2",params![c.agent_reservation_id,c.predecessor.stopped_receipt_digest],|r|r.get(0))?
            }else{
                tx.query_row("SELECT json_extract(payload,'$.processDigest') FROM owner_shared_successor_observations WHERE attempt_id=?1 AND digest=?2",params![c.predecessor.command_id,c.predecessor.stopped_receipt_digest],|r|r.get(0))?
            };
            if process!=c.predecessor.process_digest{return Err(Error::LocalAuthority)}
            let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM owner_shared_successor_attempts WHERE child_id=?1 AND process_attempt>=?2)",params![c.agent_reservation_id,c.process_attempt],|r|r.get(0))?;
            if exists{return Err(Error::Conflict)}
            bounded_row(&tx,"owner_shared_successor_attempts","id",&c.command_id,100_000)?;
            let issued=revision(&tx)?;
            let incarnation=format!("successor_{issued}_{}",&p.digest[..32]);
            let binding=canonical::digest(&json!({"child":c.agent_reservation_id,"assignmentId":c.assignment_id,"assignmentGeneration":c.assignment_generation,"commandDigest":b.digest,"providerDigest":b.provider}))?;
            tx.execute("INSERT INTO owner_shared_successor_attempts VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",params![c.command_id,c.agent_reservation_id,p.enrollment.profile.digest,c.process_attempt,c.predecessor.command_id,c.predecessor.command_digest,c.predecessor.stopped_receipt_digest,c.start_intent_id,canonical::encode(&serde_json::to_value(c)?)?,p.digest,c.original_runtime_digest,binding,b.provider,expected.observed_ledger_revision,issued,incarnation])?;
            p.check()?;after(&p.enrollment.profile,&registry,expires)?;tx.commit()?;
            p.check().map_err(|_|Error::OutcomeUnknown)?;
            Ok(SharedBudgetAttemptClaim::Issued(Box::new(SharedBudgetAttemptCustody{registry,enrollment:p.enrollment,command:p.command,expires,incarnation,digest:p.digest,provider:b.provider,issued_revision:issued,control_revision})))
        }).await?;
        after(&profile, &self.0, expires)?;
        Ok(value)
    }
}
