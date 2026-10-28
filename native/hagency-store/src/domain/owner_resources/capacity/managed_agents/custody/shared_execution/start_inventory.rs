//! Local execution history, never current readiness or permission to start.
use super::*;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedSuccessorSummary {
    pub assignment_id: String,
    pub request_id: String,
    pub agent_name: String,
    pub project_id: String,
    pub predecessor_command_id: String,
    /// Current recorded process attempt; explicit review computes the next one.
    pub process_attempt: u64,
    pub process_state: Option<String>,
    pub saved_start: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedSuccessorPage {
    pub items: Vec<SharedSuccessorSummary>,
    pub next_after: Option<String>,
}

fn summary(
    db: &Connection,
    profile: &ProfileAuthority,
    assignment: &str,
    device: &str,
    generation: u64,
) -> Result<SharedSuccessorSummary, Error> {
    let (child, digest, payload): (String, String, String) = db.query_row(
        "SELECT child_id,command_digest,command_payload FROM owner_shared_runtime_lifetimes WHERE assignment_id=?1 AND profile=?2",
        params![assignment, profile.digest], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let original: SharedRuntimeCommand = serde_json::from_str(&payload)?;
    original.validate(&profile.account)?;
    if original.assignment_id != assignment
        || original.agent_reservation_id != child
        || original.reserve_command.executor_device_id != device
        || original.reserve_command.executor_generation != generation
        || canonical::digest(&serde_json::to_value(&original)?)? != digest
    {
        return Err(Error::Conflict);
    }
    let latest: Option<(String, String, String)> = db.query_row(
        "SELECT profile,command_digest,command_payload FROM owner_shared_successor_attempts WHERE child_id=?1 ORDER BY process_attempt DESC LIMIT 1",
        [&child], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).optional()?;
    let (command_id, process_attempt, process_state) = if let Some((
        saved_profile,
        saved_digest,
        payload,
    )) = latest
    {
        let value:serde_json::Value=serde_json::from_str(&payload)?;
        if value.get("version").and_then(serde_json::Value::as_u64)==Some(3) {
            let c:SharedBudgetAttemptCommand=serde_json::from_value(value)?;
            c.validate(&profile.account)?;
            if saved_profile!=profile.digest||c.agent_reservation_id!=child||c.assignment_id!=assignment
                ||c.original_runtime_digest!=digest||canonical::digest(&serde_json::to_value(&c)?)?!=saved_digest{return Err(Error::Conflict)}
            let row:Option<(String,String,u64)>=db.query_row("SELECT payload,digest,ledger_revision FROM owner_shared_successor_observations WHERE attempt_id=?1 ORDER BY ledger_revision DESC LIMIT 1",[&c.command_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            let state=row.map(|(payload,digest,revision)|->Result<String,Error>{
                let receipt:SharedBudgetAttemptProcessReceipt=serde_json::from_str(&payload)?;
                receipt.validate_for(&c)?;
                if receipt.ledger_revision!=revision||canonical::digest(&serde_json::to_value(&receipt)?)?!=digest{return Err(Error::Conflict)}
                Ok(receipt.state)
            }).transpose()?;
            (c.command_id,c.process_attempt,state)
        } else {
        let c: SharedAttemptCommand = serde_json::from_str(&payload)?;
        c.validate(&profile.account)?;
        if saved_profile != profile.digest
            || c.agent_reservation_id != child
            || c.assignment_id != assignment
            || c.original_runtime_digest != digest
            || canonical::digest(&serde_json::to_value(&c)?)? != saved_digest
        {
            return Err(Error::Conflict);
        }
        let row: Option<(String,String,u64)> = db.query_row(
            "SELECT payload,digest,ledger_revision FROM owner_shared_successor_observations WHERE attempt_id=?1 ORDER BY ledger_revision DESC LIMIT 1",
            [&c.command_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
        ).optional()?;
        let state = row
            .map(|(payload, digest, revision)| -> Result<String, Error> {
                let receipt: SharedAttemptProcessReceipt = serde_json::from_str(&payload)?;
                receipt.validate_for(&c)?;
                if receipt.ledger_revision != revision
                    || canonical::digest(&serde_json::to_value(&receipt)?)? != digest
                {
                    return Err(Error::Conflict);
                }
                Ok(receipt.state)
            })
            .transpose()?;
        (c.command_id, c.process_attempt, state)
        }
    } else {
        let row: Option<(String,String,u64)> = db.query_row(
            "SELECT payload,digest,ledger_revision FROM owner_shared_process_observations WHERE child_id=?1 ORDER BY ledger_revision DESC LIMIT 1",
            [&child], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
        ).optional()?;
        let state = row
            .map(|(payload, digest, revision)| -> Result<String, Error> {
                let receipt: SharedProcessReceipt = serde_json::from_str(&payload)?;
                receipt.validate_for(&original)?;
                if receipt.ledger_revision != revision
                    || canonical::digest(&serde_json::to_value(&receipt)?)? != digest
                {
                    return Err(Error::Conflict);
                }
                Ok(receipt.state)
            })
            .transpose()?;
        (original.command_id.clone(), 1, state)
    };
    let saved_id: Option<String> = db.query_row(
        "SELECT id FROM owner_shared_start_intents WHERE child_id=?1 AND profile=?2 AND device_id=?3 AND device_generation=?4 AND NOT EXISTS(SELECT 1 FROM owner_shared_demand_decisions d WHERE d.start_intent_id=owner_shared_start_intents.id) ORDER BY ledger_revision DESC LIMIT 1",
        params![child,profile.digest,device,generation], |r| r.get(0),
    ).optional()?;
    if let Some(id) = &saved_id {
        let saved = start_selection::saved_start(db, profile, id)?;
        if canonical::digest(&serde_json::to_value(saved.original())?)? != digest {
            return Err(Error::Conflict);
        }
    }
    Ok(SharedSuccessorSummary {
        assignment_id: original.assignment_id,
        request_id: original.reserve_command.request_id,
        agent_name: original.reserve_command.definition.name,
        project_id: original.reserve_command.project_id,
        predecessor_command_id: command_id,
        process_attempt,
        process_state,
        saved_start: saved_id.is_some(),
    })
}

impl OwnerResourceRegistry {
    /// The trusted native caller binds these filters to its current device.
    /// Rows are descriptive and remain readable when execution policy is off.
    /// No row creates an intent, changes a hold, or attests live central policy.
    pub async fn shared_successor_inventory(
        &self,
        profile: ProfileAuthority,
        device: String,
        generation: u64,
        after_id: Option<String>,
    ) -> Result<SharedSuccessorPage, Error> {
        profile.check()?;
        identifier(&device, 128)?;
        exact(generation)?;
        if generation == 0 {
            return Err(Error::LocalAuthority);
        }
        if let Some(id) = &after_id {
            identifier(id, 128)?;
        }
        let p = profile.clone();
        let expires = p.expires;
        let registry = self.0.clone();
        let result = self.0.store.call(64*1024, move |db| {
            profile.check()?;
            if registry.closed.load(Ordering::Acquire) { return Err(Error::LocalAuthority); }
            let tx = db.db.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let ids: Vec<String> = {
                let mut query = tx.prepare("SELECT assignment_id FROM owner_shared_runtime_lifetimes WHERE profile=?1 AND json_extract(command_payload,'$.reserveCommand.executorDeviceId')=?2 AND json_extract(command_payload,'$.reserveCommand.executorGeneration')=?3 AND (?4 IS NULL OR assignment_id>?4) ORDER BY assignment_id LIMIT 17")?;
                query.query_map(params![profile.digest,device,generation,after_id], |r| r.get(0))?.collect::<Result<_,_>>()?
            };
            let has_more = ids.len() > 16;
            let items: Vec<_> = ids.iter().take(16).map(|id| summary(&tx,&profile,id,&device,generation)).collect::<Result<_,_>>()?;
            let next_after = has_more.then(|| items.last().unwrap().assignment_id.clone());
            profile.check()?;
            if registry.closed.load(Ordering::Acquire) { return Err(Error::LocalAuthority); }
            tx.commit()?;
            Ok(SharedSuccessorPage { items, next_after })
        }).await?;
        after(&p, &self.0, expires)?;
        Ok(result)
    }
}
