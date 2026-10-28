//! Evidence-only sink retained by the original process task. Cleanup survives
//! profile retirement; it can never create/requalify a process or release holds.
use super::*;
use hagency_runtime::owned::{
    ManagedProcessObservation, ManagedProcessSource, ManagedProcessState,
};

pub struct ManagedRuntimeEvidenceSink {
    custody: ManagedRuntimeCustody,
    source: ManagedProcessSource,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedRuntimeRecordedProcess {
    pub observation_id: String,
    pub ledger_revision: u64,
    pub state: String,
    pub receipt: Option<ManagedRuntimeReceipt>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Evidence {
    child: String,
    incarnation: String,
    provision_digest: String,
    state: String,
    provider_digest: Option<String>,
    runtime_digest: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stored {
    evidence: Evidence,
    record: ManagedRuntimeRecordedProcess,
}
fn bound(db: &Connection, c: &ManagedRuntimeCustody) -> Result<(), Error> {
    let row:(String,String,String,u64,String)=db.query_row("SELECT profile,command_digest,incarnation,issued_revision,binding_digest FROM owner_managed_runtime_lifetimes WHERE child_id=?1",[&c.scope.child],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
    if row.0 != c.scope.profile.digest
        || row.1 != canonical::digest(&serde_json::to_value(&c.command)?)?
        || row.2 != c.incarnation
        || row.3 != c.scope.issued_revision
        || row.4 != c.scope.binding
    {
        return Err(Error::LocalAuthority);
    }
    Ok(())
}
fn live(db: &Connection, c: &ManagedRuntimeCustody) -> Result<(), Error> {
    c.scope.check(&c.scope.registry)?;
    c.enrollment.check()?;
    bound(db, c)?;
    if graphs::now_ms()? >= c.command.expires_at_ms {
        return Err(Error::LocalAuthority);
    }
    let b = authority::original(db, &c.enrollment, &c.scope.child, false)?;
    if b.provider != c.provider_digest
        || read_resource(db, &c.command.reserve_command.resource_id)?.framework != "codex"
    {
        return Err(Error::LocalAuthority);
    }
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM owner_managed_runtime_stops WHERE child_id=?1)",
        [&c.scope.child],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(Error::LocalAuthority);
    }
    Ok(())
}
fn current_stop(db: &Connection, child: &str) -> Result<Option<ManagedRuntimeCommand>, Error> {
    let s: Option<String> = db
        .query_row(
            "SELECT command_payload FROM owner_managed_runtime_stops WHERE child_id=?1",
            [child],
            |r| r.get(0),
        )
        .optional()?;
    s.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
}
fn append(
    db: &Connection,
    c: &ManagedRuntimeCustody,
    evidence: Evidence,
) -> Result<ManagedRuntimeRecordedProcess, Error> {
    bound(db, c)?;
    let command = current_stop(db, &c.scope.child)?
        .unwrap_or_else(|| ManagedRuntimeCommand::Provision(c.command.clone()));
    let hash = canonical::digest(
        &json!({"evidence":evidence,"commandDigest":canonical::digest(&serde_json::to_value(&command)?)?}),
    )?;
    let id = format!("mro_{hash}");
    let old: Option<String> = db
        .query_row(
            "SELECT payload FROM owner_managed_runtime_observations WHERE id=?1",
            [&id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(old) = old {
        return Ok(serde_json::from_str::<Stored>(&old)?.record);
    }
    let mut query=db.prepare("SELECT payload FROM owner_managed_runtime_observations WHERE child_id=?1 ORDER BY ledger_revision DESC LIMIT 256")?;
    let mut rows = query.query([&c.scope.child])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        count += 1;
        let prior: Stored = serde_json::from_str(&row.get::<_, String>(0)?)?;
        if prior.evidence.state == "stopped" && evidence.state != "stopped" {
            return Err(Error::Conflict);
        }
    }
    if count >= 256 {
        return Err(Error::Capacity);
    }
    drop(rows);
    drop(query);
    let revision = revision(db)?;
    let receipt =
        if evidence.state == "stopped" && matches!(command, ManagedRuntimeCommand::Provision(_)) {
            None
        } else {
            let state = if evidence.state == "qualified" {
                "qualified"
            } else if evidence.state == "stopped" {
                "stopped"
            } else {
                "unknown"
            };
            let value = ManagedRuntimeReceipt {
                version: 1,
                observation_id: id.clone(),
                command_id: command.id().into(),
                command_digest: canonical::digest(&serde_json::to_value(&command)?)?,
                assignment_id: c.command.assignment_id.clone(),
                agent_reservation_id: c.scope.child.clone(),
                reserve_command_digest: c.command.reserve_command_digest.clone(),
                reserve_receipt_digest: c.command.reserve_receipt_digest.clone(),
                effect_id: c.scope.target.clone(),
                ledger_revision: revision,
                state: state.into(),
                process_state: match state {
                    "qualified" => "live",
                    "stopped" => "stopped",
                    _ => "unknown",
                }
                .into(),
                runtime_digest: evidence.runtime_digest.clone(),
            };
            value.validate_for(&command)?;
            Some(value)
        };
    let result = ManagedRuntimeRecordedProcess {
        observation_id: id.clone(),
        ledger_revision: revision,
        state: evidence.state.clone(),
        receipt,
    };
    let value = serde_json::to_value(Stored {
        evidence,
        record: result.clone(),
    })?;
    db.execute(
        "INSERT INTO owner_managed_runtime_observations VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            id,
            c.scope.child,
            command.id(),
            canonical::encode(&value)?,
            canonical::digest(&value)?,
            revision
        ],
    )?;
    // The checkpoint admits no model call. Process evidence never changes the
    // original effect into legacy `qualified` or settles any existing usage.
    Ok(result)
}
impl OwnerResourceRegistry {
    /// Rechecks the original process binding and current authority. This cannot
    /// recreate issuance or extend the fixed monotonic lifetime.
    pub async fn check_managed_runtime_process(
        &self,
        sink: &ManagedRuntimeEvidenceSink,
    ) -> Result<(), Error> {
        if !sink.source.is_live() {
            return Err(Error::LocalAuthority);
        }
        self.check_managed_runtime_custody(&sink.custody).await?;
        if !sink.source.is_live() {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
    /// Lost startup acknowledgement never proves absence. Persist only unknown
    /// under the consumed original one-shot custody, without minting a process
    /// observation or releasing capacity.
    pub async fn record_managed_runtime_unresolved(
        &self,
        custody: ManagedRuntimeCustody,
    ) -> Result<ManagedRuntimeRecordedProcess, Error> {
        if !Arc::ptr_eq(&custody.scope.registry, &self.0) || self.0.closed.load(Ordering::Acquire) {
            return Err(Error::LocalAuthority);
        }
        let registry = self.0.clone();
        self.0
            .store
            .call(32 * 1024, move |db| {
                if registry.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                let tx = db
                    .db
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                let evidence = Evidence {
                    child: custody.scope.child.clone(),
                    incarnation: custody.incarnation.clone(),
                    provision_digest: canonical::digest(&serde_json::to_value(&custody.command)?)?,
                    state: "unknown".into(),
                    provider_digest: None,
                    runtime_digest: None,
                };
                let result = append(&tx, &custody, evidence)?;
                tx.commit()?;
                Ok(result)
            })
            .await
    }

    pub async fn check_managed_runtime_custody(
        &self,
        c: &ManagedRuntimeCustody,
    ) -> Result<(), Error> {
        c.scope.check(&self.0)?;
        let scope = c.scope.clone();
        let command = c.command.clone();
        let incarnation = c.incarnation.clone();
        let provider = c.provider_digest.clone();
        let enrollment = c.enrollment.clone();
        self.0
            .store
            .call(32 * 1024, move |db| {
                live(
                    &db.db,
                    &ManagedRuntimeCustody {
                        scope,
                        command,
                        incarnation,
                        provider_digest: provider,
                        enrollment,
                    },
                )
            })
            .await?;
        c.scope.check(&self.0)
    }
    /// Consumes original one-shot custody once. Binding grants no permission to
    /// launch; it retains only the source needed for late supervised evidence.
    pub fn bind_managed_runtime_process(
        &self,
        c: ManagedRuntimeCustody,
        source: ManagedProcessSource,
    ) -> Result<ManagedRuntimeEvidenceSink, Error> {
        if !Arc::ptr_eq(&c.scope.registry, &self.0) || self.0.closed.load(Ordering::Acquire) {
            return Err(Error::LocalAuthority);
        }
        Ok(ManagedRuntimeEvidenceSink { custody: c, source })
    }
    pub async fn record_managed_runtime_process(
        &self,
        sink: &ManagedRuntimeEvidenceSink,
        observed: ManagedProcessObservation,
    ) -> Result<ManagedRuntimeRecordedProcess, Error> {
        let c = &sink.custody;
        if !Arc::ptr_eq(&c.scope.registry, &self.0)
            || self.0.closed.load(Ordering::Acquire)
            || !sink.source.same(observed.source())
        {
            return Err(Error::LocalAuthority);
        }
        let qualified = observed.state() == ManagedProcessState::Qualified;
        if qualified
            && (!sink.source.is_live()
                || observed.provider_digest() != Some(c.provider_digest.as_str()))
        {
            return Err(Error::LocalAuthority);
        }
        if qualified {
            c.scope.check(&self.0)?;
        }
        let evidence = Evidence {
            child: c.scope.child.clone(),
            incarnation: c.incarnation.clone(),
            provision_digest: canonical::digest(&serde_json::to_value(&c.command)?)?,
            state: match observed.state() {
                ManagedProcessState::Qualified => "qualified",
                ManagedProcessState::Stopped => "stopped",
                ManagedProcessState::Unknown => "unknown",
            }
            .into(),
            provider_digest: observed.provider_digest().map(str::to_owned),
            runtime_digest: observed.policy_digest().map(str::to_owned),
        };
        let scope = c.scope.clone();
        let command = c.command.clone();
        let incarnation = c.incarnation.clone();
        let provider = c.provider_digest.clone();
        let enrollment = c.enrollment.clone();
        let source = sink.source.clone();
        let registry = self.0.clone();
        let result = self
            .0
            .store
            .call(32 * 1024, move |db| {
                if registry.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                let c = ManagedRuntimeCustody {
                    scope,
                    command,
                    incarnation,
                    provider_digest: provider,
                    enrollment,
                };
                let tx = db
                    .db
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                if qualified {
                    live(&tx, &c)?;
                    if !source.is_live() {
                        return Err(Error::LocalAuthority);
                    }
                }
                let result = append(&tx, &c, evidence)?;
                if qualified {
                    live(&tx, &c)?;
                    if !source.is_live() {
                        return Err(Error::LocalAuthority);
                    }
                }
                tx.commit()?;
                Ok(result)
            })
            .await?;
        if qualified && (c.scope.check(&self.0).is_err() || !sink.source.is_live()) {
            return Err(Error::OutcomeUnknown);
        }
        if self.0.closed.load(Ordering::Acquire) {
            return Err(Error::OutcomeUnknown);
        }
        Ok(result)
    }
}

impl OwnerResourceRegistry {
    /// Current exact cleanup delivery may publish previously retained process
    /// evidence. A missing observer remains unknown; only an unissued tombstone
    /// that recorded the original pending effect can establish not_started.
    pub async fn collect_managed_runtime_stop(
        &self,
        p: VerifiedManagedRuntimeDelivery,
    ) -> Result<ManagedRuntimeReceipt, Error> {
        p.check()?;
        if !matches!(p.command, ManagedRuntimeCommand::Stop(_)) {
            return Err(Error::LocalAuthority);
        }
        let profile = p.enrollment.profile.clone();
        let deadline = p.deadline();
        let registry = self.0.clone();
        let receipt=self.0.store.call(32*1024,move|db| {
            p.check()?;if registry.closed.load(Ordering::Acquire){return Err(Error::LocalAuthority);}
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;original(&tx,&p)?;let c=p.command.original();
            let (issued,incarnation,source):(Option<u64>,Option<String>,String)=tx.query_row("SELECT issued_revision,incarnation,source_effect_state FROM owner_managed_runtime_lifetimes WHERE child_id=?1",[&c.agent_reservation_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            let saved:String=tx.query_row("SELECT command_digest FROM owner_managed_runtime_stops WHERE child_id=?1",[&c.agent_reservation_id],|r|r.get(0))?;
            if saved!=p.digest {return Err(Error::Conflict);}
            let previous:Option<String>=tx.query_row("SELECT payload FROM owner_managed_runtime_observations WHERE child_id=?1 ORDER BY ledger_revision DESC LIMIT 1",[&c.agent_reservation_id],|r|r.get(0)).optional()?;
            let previous=previous.map(|s|serde_json::from_str::<Stored>(&s)).transpose()?;
            let state=if previous.as_ref().is_some_and(|s|s.evidence.state=="stopped") {"stopped"}else if issued.is_none()&&source=="pending" {"not_started"}else {"unknown"};
            if let Some(r)=previous.as_ref().and_then(|s|s.record.receipt.as_ref()).filter(|r|r.command_id==p.command.id()&&r.state==state) {p.check()?;return Ok(r.clone());}
            let hash=canonical::digest(&json!({"commandDigest":p.digest,"state":state,"sourceObservationId":previous.as_ref().map(|s|s.record.observation_id.as_str())}))?;
            let id=format!("mrr_{hash}");let revision=revision(&tx)?;
            let result=ManagedRuntimeReceipt {version:1,observation_id:id.clone(),command_id:p.command.id().into(),command_digest:p.digest.clone(),assignment_id:c.assignment_id.clone(),agent_reservation_id:c.agent_reservation_id.clone(),reserve_command_digest:c.reserve_command_digest.clone(),reserve_receipt_digest:c.reserve_receipt_digest.clone(),effect_id:format!("{}_provision",c.agent_reservation_id),ledger_revision:revision,state:state.into(),process_state:state.into(),runtime_digest:None};result.validate_for(&p.command)?;
            let evidence=Evidence {child:c.agent_reservation_id.clone(),incarnation:incarnation.unwrap_or_else(||"not_issued".into()),provision_digest:canonical::digest(&serde_json::to_value(c)?)?,state:state.into(),provider_digest:None,runtime_digest:None};
            let record=ManagedRuntimeRecordedProcess {observation_id:id.clone(),ledger_revision:revision,state:state.into(),receipt:Some(result.clone())};let v=serde_json::to_value(Stored{evidence,record})?;
            tx.execute("INSERT INTO owner_managed_runtime_observations VALUES(?1,?2,?3,?4,?5,?6)",params![id,c.agent_reservation_id,p.command.id(),canonical::encode(&v)?,canonical::digest(&v)?,revision])?;
            p.check()?;tx.commit()?;p.check().map_err(|_|Error::OutcomeUnknown)?;Ok(result)
        }).await?;
        after(&profile, &self.0, deadline)?;
        Ok(receipt)
    }
    /// Read-only original receipt recovery. No refreshed process custody or lease.
    pub async fn recover_managed_runtime_receipt(
        &self,
        profile: ProfileAuthority,
        device: String,
        command_id: String,
        command_digest: String,
    ) -> Result<Option<ManagedRuntimeReceipt>, Error> {
        profile.check()?;
        identifier(&device, 128)?;
        identifier(&command_id, 128)?;
        hash(&command_digest)?;
        let caller = profile.clone();
        let registry = self.0.clone();
        let result=self.0.store.call(32*1024,move|db| {
            profile.check()?;if registry.closed.load(Ordering::Acquire){return Err(Error::LocalAuthority);}
            let lifetime:Option<(String,String,String)>=db.db.query_row("SELECT l.profile,l.command_payload,l.command_digest FROM owner_managed_runtime_lifetimes l LEFT JOIN owner_managed_runtime_stops s ON s.child_id=l.child_id WHERE l.command_id=?1 OR s.command_id=?1",[&command_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            let Some((owner,payload,original_digest))=lifetime else {return Ok(None);};
            let c:ManagedRuntimeProvisionCommand=serde_json::from_str(&payload)?;
            if owner!=profile.digest||c.reserve_command.executor_device_id!=device {return Err(Error::LocalAuthority);}
            let expected=if c.command_id==command_id {original_digest}else{db.db.query_row("SELECT command_digest FROM owner_managed_runtime_stops WHERE command_id=?1",[&command_id],|r|r.get(0))?};
            if expected!=command_digest {return Err(Error::Conflict);}
            let value:Option<String>=db.db.query_row("SELECT payload FROM owner_managed_runtime_observations WHERE command_id=?1 AND json_type(payload,'$.record.receipt')='object' ORDER BY ledger_revision DESC LIMIT 1",[&command_id],|r|r.get(0)).optional()?;
            let result=value.map(|s|serde_json::from_str::<Stored>(&s)).transpose()?.and_then(|s|s.record.receipt);profile.check()?;Ok(result)
        }).await?;
        caller.check()?;
        if self.0.closed.load(Ordering::Acquire) {
            return Err(Error::LocalAuthority);
        }
        Ok(result)
    }
}
