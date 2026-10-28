//! Native accepted-start journal. Deserialized wire inputs never construct the
//! opaque handle; original saved bytes are recovered before re-observing budget.
use super::*;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedAttemptStartInput {
    /// Explicit owner opt-in, pinned in this successor start and command.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub web_search: bool,
    #[serde(default, skip_serializing_if="Option::is_none")]
    pub tool_policy: Option<SharedAgentToolPolicy>,
    pub start_intent_id: String,
    pub assignment_id: String,
    pub predecessor: SharedAttemptPredecessor,
    pub budget_observation: SharedBudgetObservation,
}
/// Original immutable request custody, not execution or activity authority.
pub struct SharedAttemptStartIntent {
    pub(super) input: SharedAttemptStartInput,
    pub(super) original: SharedRuntimeCommand,
}

#[cfg(test)]
mod web_tests {
    use super::*;
    #[test]
    fn shared_web_command_must_match_the_saved_owner_opt_in() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!("management-shared-attempt-v2.json")).unwrap();
        let mut command: SharedAttemptCommand = serde_json::from_value(fixture["command"].clone()).unwrap();
        assert!(!command.web_search);
        assert_eq!(canonical::digest(&serde_json::to_value(&command).unwrap()).unwrap(), fixture["commandDigest"]);
        let mut intent = SharedAttemptStartIntent {
            input: SharedAttemptStartInput {
                web_search: false, tool_policy: None,
                start_intent_id: command.start_intent_id.clone(),
                assignment_id: command.assignment_id.clone(),
                predecessor: command.predecessor.clone(),
                budget_observation: command.budget_observation.clone(),
            },
            original: command.original_runtime_command.clone(),
        };
        intent.matches_command(&command).unwrap();
        command.web_search = true;
        assert!(intent.matches_command(&command).is_err());
        intent.input.web_search = true;
        intent.matches_command(&command).unwrap();
        command.web_search = false;
        assert!(intent.matches_command(&command).is_err());
    }
}
impl SharedAttemptStartIntent {
    pub fn input(&self) -> &SharedAttemptStartInput {
        &self.input
    }
    pub fn original(&self) -> &SharedRuntimeCommand {
        &self.original
    }
    pub fn matches_command(&self, c: &SharedAttemptCommand) -> Result<(), Error> {
        if c.tool_policy != self.input.tool_policy
            || c.web_search != self.input.web_search
            || c.start_intent_id != self.input.start_intent_id
            || c.assignment_id != self.input.assignment_id
            || c.original_runtime_digest
                != canonical::digest(&serde_json::to_value(&self.original)?)?
            || canonical::digest(&serde_json::to_value(&c.predecessor)?)?
                != canonical::digest(&serde_json::to_value(&self.input.predecessor)?)?
            || canonical::digest(&serde_json::to_value(&c.budget_observation)?)?
                != canonical::digest(&serde_json::to_value(&self.input.budget_observation)?)?
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}
impl OwnerResourceRegistry {
    /// `start_id` belongs to one accepted native startup/explicit-start action.
    /// Reusing it returns its exact old request, never a fresh budget snapshot.
    /// There is deliberately no per-poll auto-create API.
    pub async fn prepare_shared_attempt_start(
        &self,
        enrollment: VerifiedManagementEnrollment,
        original: SharedRuntimeCommand,
        predecessor: Option<SharedAttemptCommand>,
        start_id: String,
    ) -> Result<SharedAttemptStartIntent, Error> {
        enrollment.check()?;
        original.validate(&enrollment.profile.account)?;
        identifier(&start_id, 128)?;
        let p = enrollment.profile.clone();
        let expires = enrollment.expires;
        let registry = self.0.clone();
        let value = self
            .0
            .store
            .call(64 * 1024, move |db| {
                enrollment.check()?;
                if registry.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                let tx = db
                    .db
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                demand::require_explicit_start(&tx, &start_id)?;
                let value =
                    prepare_in_transaction(&tx, &enrollment, original, predecessor, start_id)?;
                tx.commit()?;
                enrollment.check().map_err(|_| Error::OutcomeUnknown)?;
                Ok(value)
            })
            .await?;
        after(&p, &self.0, expires)?;
        Ok(value)
    }
}
impl OwnerResourceRegistry {
    /// Restart lookup under current same-installation cleanup authority. The
    /// returned original request remains data, not a fresh spawn/start lease.
    pub async fn recover_shared_attempt_start(
        &self,
        authority: ManagedCleanupAuthority,
        original: SharedRuntimeCommand,
        predecessor: Option<SharedAttemptCommand>,
    ) -> Result<Option<SharedAttemptStartIntent>, Error> {
        authority.check(&self.0)?;
        original.validate(&authority.enrollment.profile.account)?;
        if authority.child != original.agent_reservation_id {
            return Err(Error::LocalAuthority);
        }
        let p = authority.enrollment.profile.clone();
        let expires = authority.deadline();
        let registry = self.0.clone();
        let result=self.0.store.call(64*1024,move|db|{
            authority.check(&registry)?;let binding=authority::cleanup(&db.db,&authority)?;
            if binding.digest!=original.reserve_command_digest{return Err(Error::LocalAuthority)}
            let original_digest=canonical::digest(&serde_json::to_value(&original)?)?;
            let predecessor_digest=predecessor.as_ref().map(|c| -> Result<String,Error>{c.validate(&authority.enrollment.profile.account)?;if c.original_runtime_digest!=original_digest{return Err(Error::Conflict)}Ok(canonical::digest(&serde_json::to_value(c)?)?)}).transpose()?.unwrap_or_else(||original_digest.clone());
            let row:Option<(String,String,String,String)>=db.db.query_row("SELECT profile,original_runtime_digest,input_payload,input_digest FROM owner_shared_start_intents WHERE child_id=?1 AND predecessor_digest=?2 AND NOT EXISTS(SELECT 1 FROM owner_shared_demand_decisions d WHERE d.start_intent_id=owner_shared_start_intents.id)",params![authority.child,predecessor_digest],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
            let Some((profile,saved_original,payload,digest))=row else {return Ok(None)};
            let input:SharedAttemptStartInput=serde_json::from_str(&payload)?;
            if profile!=authority.enrollment.profile.digest||saved_original!=original_digest||input.assignment_id!=original.assignment_id||input.predecessor.command_digest!=predecessor_digest||canonical::digest(&serde_json::to_value(&input)?)?!=digest{return Err(Error::Conflict)}
            authority.check(&registry)?;Ok(Some(SharedAttemptStartIntent{input,original}))
        }).await?;
        after(&p, &self.0, expires)?;
        Ok(result)
    }
}
impl OwnerResourceRegistry {
    pub async fn require_shared_attempt_start(
        &self,
        authority: ManagedCleanupAuthority,
        command: SharedAttemptCommand,
    ) -> Result<bool, Error> {
        authority.check(&self.0)?;
        command.validate(&authority.enrollment.profile.account)?;
        if authority.child != command.agent_reservation_id {
            return Err(Error::LocalAuthority);
        }
        let p = authority.enrollment.profile.clone();
        let expires = authority.deadline();
        let registry = self.0.clone();
        let found=self.0.store.call(64*1024,move|db|{
            authority.check(&registry)?;authority::cleanup(&db.db,&authority)?;
            let row:Option<(String,String,String,String)>=db.db.query_row("SELECT profile,original_runtime_payload,input_payload,input_digest FROM owner_shared_start_intents WHERE id=?1 AND child_id=?2",params![command.start_intent_id,command.agent_reservation_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
            let Some((profile,original,input,digest))=row else{return Ok(false)};
            let saved=SharedAttemptStartIntent{input:serde_json::from_str(&input)?,original:serde_json::from_str(&original)?};
            if profile!=authority.enrollment.profile.digest||canonical::digest(&serde_json::to_value(saved.input())?)?!=digest{return Err(Error::Conflict)}
            saved.matches_command(&command)?;authority.check(&registry)?;Ok(true)
        }).await?;
        after(&p, &self.0, expires)?;
        Ok(found)
    }
}

// Same transaction helper used by the explicit-start API and the isolated
// demand journal. The caller owns commit; no second revision follows this
// snapshot before its original start bytes are saved.
pub(super) fn prepare_in_transaction(
    tx: &Connection,
    enrollment: &VerifiedManagementEnrollment,
    original: SharedRuntimeCommand,
    predecessor: Option<SharedAttemptCommand>,
    start_id: String,
) -> Result<SharedAttemptStartIntent, Error> {
    let original_digest = canonical::digest(&serde_json::to_value(&original)?)?;
    let predecessor_digest = predecessor
        .as_ref()
        .map(|c| -> Result<String, Error> { Ok(canonical::digest(&serde_json::to_value(c)?)?) })
        .transpose()?
        .unwrap_or_else(|| original_digest.clone());
    let b = authority::original(tx, enrollment, &original.agent_reservation_id, false)?;
    if b.digest != original.reserve_command_digest {
        return Err(Error::LocalAuthority);
    }
    let saved:Option<(String,String,u64,String,String,String,String)>=tx.query_row("SELECT profile,device_id,device_generation,original_runtime_digest,predecessor_digest,input_payload,input_digest FROM owner_shared_start_intents WHERE id=?1",[&start_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional()?;
    if let Some((profile, device, generation, old_original, old_predecessor, input, saved_digest)) =
        saved
    {
        if profile != enrollment.profile.digest
            || device != enrollment.executor_device_id
            || generation != enrollment.executor_generation
            || old_original != original_digest
            || old_predecessor != predecessor_digest
        {
            return Err(Error::Conflict);
        }
        let input: SharedAttemptStartInput = serde_json::from_str(&input)?;
        if canonical::digest(&serde_json::to_value(&input)?)? != saved_digest
            || input.start_intent_id != start_id
            || input.assignment_id != original.assignment_id
            || input.predecessor.command_digest != predecessor_digest
        {
            return Err(Error::Conflict);
        }
        enrollment.check()?;
        return Ok(SharedAttemptStartIntent { input, original });
    }
    // A new v3 journal for this predecessor cannot be bypassed by downgrading
    // to a v2 caller, including builds with v3 issuance disabled.
    let budget_intent:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM owner_shared_budget_start_intents WHERE id=?1 OR (child_id=?2 AND predecessor_digest=?3))",params![start_id,original.agent_reservation_id,predecessor_digest],|r|r.get(0))?;
    if budget_intent{return Err(Error::Conflict)}
    // Advance before snapshot so journal persistence itself does not
    // immediately invalidate the exact observed global ledger revision.
    let rev = revision(tx)?;
    let budget = successor::snapshot_for_head(tx, enrollment, &original, predecessor.as_ref())?;
    if budget.state != SharedSuccessorBudgetState::Available
        || budget.predecessor_command_digest != predecessor_digest
        || budget.observed_ledger_revision != rev
    {
        return Err(Error::Capacity);
    }
    let (command_id, number, process) = if let Some(head) = &predecessor {
        let process:String=tx.query_row("SELECT json_extract(payload,'$.processDigest') FROM owner_shared_successor_observations WHERE attempt_id=?1 AND digest=?2",params![head.command_id,budget.stopped_receipt_digest],|r|r.get(0))?;
        (head.command_id.clone(), head.process_attempt, process)
    } else {
        let process:String=tx.query_row("SELECT json_extract(payload,'$.processDigest') FROM owner_shared_process_observations WHERE child_id=?1 AND digest=?2",params![original.agent_reservation_id,budget.stopped_receipt_digest],|r|r.get(0))?;
        (original.command_id.clone(), 1, process)
    };
    let input = SharedAttemptStartInput {
        tool_policy: owner_tool_policy(tx, &original)?,
        web_search: owner_live_web(tx, &original),
        start_intent_id: start_id.clone(),
        assignment_id: original.assignment_id.clone(),
        predecessor: SharedAttemptPredecessor {
            command_id,
            command_digest: predecessor_digest.clone(),
            process_attempt: number,
            stopped_receipt_digest: budget.stopped_receipt_digest,
            process_digest: process,
        },
        budget_observation: SharedBudgetObservation {
            observed_ledger_revision: rev,
            allocated_tokens: budget.allocated_tokens,
            known_consumed_tokens: budget.known_consumed_tokens,
            retained_tokens: budget.retained_tokens,
            remaining_tokens: budget.remaining_tokens,
            usage_complete: budget.usage_complete,
            liability_digest: budget.liability_digest,
        },
    };
    let payload = canonical::encode(&serde_json::to_value(&input)?)?;
    let input_digest = canonical::digest(&serde_json::to_value(&input)?)?;
    bounded_row(tx, "owner_shared_start_intents", "id", &start_id, 100_000)?;
    tx.execute(
        "INSERT INTO owner_shared_start_intents VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            start_id,
            original.agent_reservation_id,
            enrollment.profile.digest,
            enrollment.executor_device_id,
            enrollment.executor_generation,
            predecessor_digest,
            original_digest,
            canonical::encode(&serde_json::to_value(&original)?)?,
            predecessor
                .map(|c| -> Result<String, Error> {
                    Ok(canonical::encode(&serde_json::to_value(c)?)?)
                })
                .transpose()?,
            payload,
            input_digest,
            rev
        ],
    )?;
    enrollment.check()?;
    Ok(SharedAttemptStartIntent { input, original })
}
