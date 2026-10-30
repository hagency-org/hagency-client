//! One-shot successor custody. Actual-process observations are separate from
//! model-call authority, which requires the current full Seat budget gate.
use super::*;

pub const SHARED_ATTEMPT_CAPABILITY: &str = "hagency.managed_agents.execution.v2";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedAttemptPredecessor {
    pub command_id: String,
    pub command_digest: String,
    pub process_attempt: u64,
    pub stopped_receipt_digest: String,
    pub process_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetObservation {
    pub observed_ledger_revision: u64,
    pub allocated_tokens: u64,
    pub known_consumed_tokens: u64,
    pub retained_tokens: u64,
    pub remaining_tokens: u64,
    pub usage_complete: bool,
    pub liability_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedAttemptCommand {
    /// Explicit owner opt-in, pinned in this successor start and command.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub web_search: bool,
    #[serde(default, skip_serializing_if="Option::is_none")]
    pub tool_policy: Option<SharedAgentToolPolicy>,
    pub version: u16,
    pub capability: String,
    pub operation: String,
    pub command_id: String,
    pub assignment_id: String,
    pub assignment_generation: u64,
    pub agent_reservation_id: String,
    pub process_attempt: u64,
    pub original_runtime_command: SharedRuntimeCommand,
    pub original_runtime_digest: String,
    pub predecessor: SharedAttemptPredecessor,
    pub budget_observation: SharedBudgetObservation,
    pub start_intent_id: String,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
}
impl SharedAttemptCommand {
    pub fn validate(&self, owner: &str) -> Result<(), Error> {
        self.original_runtime_command.validate(owner)?;
        if let Some(policy) = &self.tool_policy { policy.validate()?; }
        for id in [
            &self.command_id,
            &self.start_intent_id,
            &self.predecessor.command_id,
        ] {
            identifier(id, 128)?;
        }
        for h in [
            &self.original_runtime_digest,
            &self.predecessor.command_digest,
            &self.predecessor.stopped_receipt_digest,
            &self.predecessor.process_digest,
            &self.budget_observation.liability_digest,
        ] {
            hash(h)?;
        }
        let original = &self.original_runtime_command;
        let b = &self.budget_observation;
        for n in [
            self.process_attempt,
            self.predecessor.process_attempt,
            self.issued_at_ms,
            self.expires_at_ms,
            b.observed_ledger_revision,
            b.allocated_tokens,
            b.remaining_tokens,
        ] {
            exact(n)?;
        }
        if self.version != 2
            || self.capability != SHARED_ATTEMPT_CAPABILITY
            || self.operation != "agent.execution.provision"
            || self.assignment_generation != 1
            || self.assignment_id != original.assignment_id
            || self.agent_reservation_id != original.agent_reservation_id
            || self.process_attempt < 2
            || self.predecessor.process_attempt != self.process_attempt - 1
            || self.original_runtime_digest != canonical::digest(&serde_json::to_value(original)?)?
            || self.command_id == original.command_id
            || self.command_id == self.predecessor.command_id
            || self.process_attempt == 2
                && (self.predecessor.command_id != original.command_id
                    || self.predecessor.command_digest != self.original_runtime_digest)
            || b.known_consumed_tokens > JSON_SAFE_MAX
            || b.retained_tokens > JSON_SAFE_MAX
            || b.retained_tokens < b.known_consumed_tokens
            || b.allocated_tokens != original.reserve_command.requested_tokens
            || b.usage_complete && b.retained_tokens != b.known_consumed_tokens
            || b.remaining_tokens != b.allocated_tokens.saturating_sub(b.retained_tokens)
            || self.expires_at_ms <= self.issued_at_ms
            || self.expires_at_ms - self.issued_at_ms > 30_000
            || (original.reserve_command.pool_grant.is_none()
                && self.expires_at_ms > original.reserve_command.expires_at_ms)
            || serde_json::to_vec(self)?.len() > 32 * 1024
        {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedAttemptProcessReceipt {
    pub version: u16,
    pub observation_id: String,
    pub command_id: String,
    pub command_digest: String,
    pub assignment_id: String,
    pub agent_reservation_id: String,
    pub process_attempt: u64,
    pub original_runtime_digest: String,
    pub ledger_revision: u64,
    pub process_digest: Option<String>,
    pub state: String,
    pub policy_digest: Option<String>,
}
impl SharedAttemptProcessReceipt {
    pub fn validate_for(&self, c: &SharedAttemptCommand) -> Result<(), Error> {
        identifier(&self.observation_id, 128)?;
        exact(self.ledger_revision)?;
        for h in [&self.process_digest, &self.policy_digest]
            .into_iter()
            .flatten()
        {
            hash(h)?;
        }
        if self.version != 2
            || self.command_id != c.command_id
            || self.command_digest != canonical::digest(&serde_json::to_value(c)?)?
            || self.assignment_id != c.assignment_id
            || self.agent_reservation_id != c.agent_reservation_id
            || self.process_attempt != c.process_attempt
            || self.original_runtime_digest != c.original_runtime_digest
            || !matches!(self.state.as_str(), "qualified" | "stopped" | "unknown")
            || self.state == "qualified"
                && (self.process_digest.is_none() || self.policy_digest.is_none())
            || self.state == "stopped" && self.process_digest.is_none()
            || self.state != "qualified" && self.policy_digest.is_some()
        {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}

/// Fresh authenticated native delivery; history cannot construct it.
pub struct VerifiedSharedAttemptDelivery {
    enrollment: VerifiedManagementEnrollment,
    command: SharedAttemptCommand,
    digest: String,
    expires: Instant,
}
impl VerifiedSharedAttemptDelivery {
    pub fn new(
        enrollment: VerifiedManagementEnrollment,
        command: SharedAttemptCommand,
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
pub struct SharedAttemptHistory {
    pub command: SharedAttemptCommand,
    pub command_digest: String,
    pub issued_revision: u64,
}
/// Exactly one process-attempt claim. No Clone/Deserialize; actual-process
/// binding records evidence and does not create model-call authority.
pub struct SharedAttemptCustody {
    pub(super) registry: Arc<Inner>,
    pub(super) enrollment: VerifiedManagementEnrollment,
    pub(super) command: SharedAttemptCommand,
    pub(super) expires: Instant,
    pub(super) incarnation: String,
    pub(super) digest: String,
    pub(super) provider: String,
    pub(super) issued_revision: u64,
    control_revision: Option<u64>,
}
impl SharedAttemptCustody {
    /// Exact demand Run pin from the claim; descriptive cancellation scope only.
    pub fn demand_control_revision(&self) -> Option<u64> {
        self.control_revision
    }

    pub fn command(&self) -> &SharedAttemptCommand {
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
pub enum SharedAttemptClaim {
    Issued(Box<SharedAttemptCustody>),
    History(Box<SharedAttemptHistory>),
}

fn history(
    db: &Connection,
    profile: &str,
    c: &SharedAttemptCommand,
) -> Result<Option<SharedAttemptHistory>, Error> {
    let row:Option<(String,String,String,u64)>=db.query_row("SELECT profile,command_payload,command_digest,issued_revision FROM owner_shared_successor_attempts WHERE id=?1",[&c.command_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let Some((saved_profile, payload, digest, issued_revision)) = row else {
        return Ok(None);
    };
    let saved: SharedAttemptCommand = serde_json::from_str(&payload)?;
    if saved_profile != profile
        || canonical::digest(&serde_json::to_value(&saved)?)? != digest
        || canonical::digest(&serde_json::to_value(c)?)? != digest
    {
        return Err(Error::Conflict);
    }
    Ok(Some(SharedAttemptHistory {
        command: saved,
        command_digest: digest,
        issued_revision,
    }))
}

impl OwnerResourceRegistry {
    /// Historical command custody under fresh same-installation authorization.
    /// No process right is reconstructed, even after a generation change.
    pub async fn shared_successor_history(
        &self,
        authority: ManagedCleanupAuthority,
        command: SharedAttemptCommand,
    ) -> Result<Option<SharedAttemptHistory>, Error> {
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

    pub async fn claim_shared_successor(
        &self,
        p: VerifiedSharedAttemptDelivery,
    ) -> Result<SharedAttemptClaim, Error> {
        p.check()?;
        let profile = p.enrollment.profile.clone();
        let expires = p.expires;
        let registry = self.0.clone();
        let value=self.0.store.call(64*1024,move|db|{
            p.check()?;after(&p.enrollment.profile,&registry,expires)?;
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let c=&p.command;
            authority::original(&tx,&p.enrollment,&c.agent_reservation_id,true)?;
            if let Some(old)=history(&tx,&p.enrollment.profile.digest,c)?{p.check()?;after(&p.enrollment.profile,&registry,expires)?;return Ok(SharedAttemptClaim::History(Box::new(old)))}
            attempt_fence::require_unfenced(&tx,c)?;
            if owner_tool_policy(&tx,&c.original_runtime_command)? != c.tool_policy {return Err(Error::LocalAuthority)}
            let control_revision=demand::attempt_control_revision(&tx, &p.enrollment.profile.digest, c)?;
            let now=graphs::now_ms()?;
            let b=authority::original(&tx,&p.enrollment,&c.agent_reservation_id,false)?;
            if now>=authority::lifetime(&tx,&b,now)?{return Err(Error::LocalAuthority)}
            let current=if c.process_attempt==2 {successor::snapshot(&tx,&p.enrollment,&c.original_runtime_command)?}else{
                let payload:String=tx.query_row("SELECT command_payload FROM owner_shared_successor_attempts WHERE id=?1 AND child_id=?2",params![c.predecessor.command_id,c.agent_reservation_id],|r|r.get(0))?;
                let predecessor:SharedAttemptCommand=serde_json::from_str(&payload)?;
                if predecessor.process_attempt!=c.predecessor.process_attempt||canonical::digest(&serde_json::to_value(&predecessor)?)?!=c.predecessor.command_digest{return Err(Error::Conflict)}
                successor::snapshot_for_head(&tx,&p.enrollment,&c.original_runtime_command,Some(&predecessor))?
            };
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
            let process:String=if c.process_attempt==2 {
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
            Ok(SharedAttemptClaim::Issued(Box::new(SharedAttemptCustody{registry,enrollment:p.enrollment,command:p.command,expires,incarnation,digest:p.digest,provider:b.provider,issued_revision:issued,control_revision})))
        }).await?;
        after(&profile, &self.0, expires)?;
        Ok(value)
    }
}
