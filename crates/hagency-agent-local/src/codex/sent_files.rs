//! Frozen original-file tools. Provider input never supplies a destination,
//! byte buffer, Server request ID, or replacement upload/send custody.
use super::*;
use crate::{
    audit::Phase,
    file_delivery::{Delivery, Manifest},
    sent_files::{Custody, Record, Sender},
};
use std::{future::Future, pin::Pin, sync::Arc};
type FileFuture<'a, T> = Pin<Box<dyn Future<Output = std::result::Result<T, ()>> + Send + 'a>>;
pub trait FileDeliveryHost: ExecutionGate {
    fn prepare<'a>(&'a self, request: &'a str, file: &'a Manifest) -> FileFuture<'a, Delivery>;
    fn chunk<'a>(
        &'a self,
        record: &'a Record,
        offset: u64,
        bytes: &'a [u8],
    ) -> FileFuture<'a, Delivery>;
    fn queue<'a>(&'a self, record: &'a Record) -> FileFuture<'a, Delivery>;
    fn status<'a>(&'a self, record: &'a Record) -> FileFuture<'a, Delivery>;
}
pub(super) struct SentFiles {
    sender: Sender,
    host: Arc<dyn FileDeliveryHost>,
    dispatch: String,
    execution: String,
    calls: BTreeMap<String, String>,
    failures: BTreeMap<String, Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendInput {
    path: String,
    filename: Option<String>,
    caption: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusInput {
    delivery_id: String,
}
pub(super) fn is_tool(tool: &Value) -> bool {
    matches!(tool.as_str(), Some("send_file" | "get_file_delivery"))
}
pub(super) fn specifications() -> Value {
    value!([
     {"type":"function","name":"send_file","description":"Freeze a regular file inside the selected workspace and ask exact owner policy/approval before staging original bytes for this request's Agent/Room/thread. Returns a delivery_id and observed state. Queued/uploaded/unknown are NOT delivered. Never create a replacement attempt to clear unknown; query the original delivery_id only. Same original file cannot regain sending custody after restart or lost response.","inputSchema":{"type":"object","properties":{"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]}},"required":["path"],"additionalProperties":false}},
     {"type":"function","name":"get_file_delivery","description":"Read only an existing original file delivery_id from this execution. No upload, continuation or resend. Only state sent with the actual Matrix event receipt proves delivery; preserve pending/unknown and report it honestly.","inputSchema":{"type":"object","properties":{"delivery_id":{"type":"string"}},"required":["delivery_id"],"additionalProperties":false}}
    ])
}
fn output(record: &Record, phase: Phase) -> Value {
    let remote = record.remote.as_ref();
    let server_state = remote.map(|r| r.state.as_str()).unwrap_or("unknown");
    let state = if record.absence.is_some() {
        "not_prepared"
    } else if phase == Phase::Unknown && !matches!(server_state, "sent" | "cancelled") {
        "unknown"
    } else {
        server_state
    };
    value!({"success":true,"contentItems":[{"type":"inputText","text":value!({"delivery_id":record.id,"file":record.manifest,"state":state,"last_observed_server_state":server_state,"matrix_event_id":remote.and_then(|r|r.matrix_event_id.as_deref()),"guidance":if state=="sent" {"Original Matrix event receipt observed."}else if state=="not_prepared" {"Server confirmed no preparation before the original execution closed. Original custody remains consumed; this is not a retry grant."}else if state=="cancelled" {"Original delivery cancelled. No message receipt observed."}else{"Not proven delivered. Query only this original delivery_id; do not send another file to clear pending/unknown. Upload acceptance is not message delivery."}}).to_string()}]})
}
fn failure(code: &str) -> Value {
    value!({"success":false,"contentItems":[{"type":"inputText","text":code}]})
}
impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> Session<R, W> {
    pub fn enable_sent_files(
        &mut self,
        sender: Sender,
        host: Arc<dyn FileDeliveryHost>,
        dispatch: String,
        execution: String,
    ) -> Result<()> {
        if self.thread.is_some() || self.sent_files.is_some() {
            return Err(Error::Protocol("sender already configured"));
        }
        key(&dispatch)?;
        key(&execution)?;
        self.sent_files = Some(SentFiles {
            sender,
            host,
            dispatch,
            execution,
            calls: BTreeMap::new(),
            failures: BTreeMap::new(),
        });
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn sent_file_call(
        &mut self,
        ledger: &mut Ledger,
        owner: &str,
        scope: &Scope,
        dispatch: &str,
        execution: &str,
        frame: &Value,
        thread: &str,
        turn: &str,
        queue: &ApprovalQueue,
    ) -> Result<()> {
        let id = frame["id"].clone();
        if !(id.is_string() || id.is_u64()) || !self.server_ids.insert(json(&id)?) {
            return Err(Error::Protocol("invalid or reused sender request ID"));
        }
        let p = &frame["params"];
        exact_scope(p, thread, turn)?;
        let call = p["callId"]
            .as_str()
            .ok_or(Error::Protocol("sender call identity missing"))?;
        key(call)?;
        let result = if let Some(files) = self.sent_files.as_mut() {
            let result = files
                .execute(
                    ledger,
                    owner,
                    scope,
                    dispatch,
                    execution,
                    p,
                    thread,
                    turn,
                    call,
                    queue,
                    &mut self.audit_pending,
                )
                .await?;
            if result["success"] == false && files.calls.contains_key(call) {
                files.failures.insert(call.into(), result.clone());
            }
            result
        } else {
            failure("sender_disabled")
        };
        self.write(value!({"id":id,"result":result})).await
    }
}
impl SentFiles {
    pub(super) fn workspace_directory(&self) -> &std::path::Path {
        self.sender.canonical_directory()
    }
    #[allow(clippy::too_many_arguments)]
    async fn execute(
        &mut self,
        ledger: &mut Ledger,
        owner: &str,
        scope: &Scope,
        dispatch: &str,
        execution: &str,
        p: &Value,
        thread: &str,
        turn: &str,
        call: &str,
        queue: &ApprovalQueue,
        pending: &mut BTreeSet<String>,
    ) -> Result<Value> {
        if dispatch != self.dispatch
            || execution != self.execution
            || !self.host.authorize(dispatch, execution).await
        {
            return Ok(failure("server_scope_unavailable"));
        }
        if matches!(
            ledger.request_disposition(scope, dispatch)?,
            crate::RequestPolicy::Deny
        ) {
            return Ok(failure("local_request_denied"));
        }
        if p.get("namespace").is_some_and(|v| !v.is_null()) {
            return Err(Error::Protocol("unexpected sender namespace"));
        }
        let arguments = json(p)?;
        if let Some(old) = self.calls.get(call) {
            if old != &arguments {
                return Err(Error::Protocol("sender call arguments changed"));
            }
        } else {
            if self.calls.len() >= 128 {
                return Err(Error::Protocol("sender call capacity"));
            }
            self.calls.insert(call.into(), arguments);
        }
        if let Some(failed) = self.failures.get(call) {
            return Ok(failed.clone());
        }
        if p["tool"] == "get_file_delivery" {
            let input = match serde_json::from_value::<StatusInput>(p["arguments"].clone()) {
                Ok(i) => i,
                Err(_) => return Ok(failure("invalid_file_delivery_request")),
            };
            let Some(record) = ledger.sent_file(scope, dispatch, execution, &input.delivery_id)?
            else {
                return Ok(failure("delivery_outside_original_execution"));
            };
            return self.observe(ledger, owner, scope, record).await;
        }
        let input = match serde_json::from_value::<SendInput>(p["arguments"].clone()) {
            Ok(i) => i,
            Err(_) => return Ok(failure("invalid_send_file_request")),
        };
        let captured = match self.sender.capture(
            ledger,
            scope,
            dispatch,
            execution,
            &input.path,
            input.filename.as_deref(),
            input.caption,
            thread,
            turn,
            call,
            now(),
        ) {
            Ok(c) => c,
            Err(crate::Error::Unknown) => {
                return Ok(failure(
                    "original_file_delivery_unresolved_query_original_id",
                ));
            }
            Err(_) => return Ok(failure("file_capture_refused")),
        };
        if let Some(record) = ledger.sent_file(scope, dispatch, execution, captured.id())? {
            return self.observe(ledger, owner, scope, record).await;
        }
        let proposal = captured.proposal().clone();
        let audit = ledger.begin_approval_audit(owner, &proposal, now())?;
        let allowed = match ledger.tool_disposition(&proposal, now()) {
            Ok(ToolPolicy::AllowWithRules { tools, directories })
                if tools.contains(&proposal.tool)
                    && directories.contains(&proposal.canonical_directory) =>
            {
                ledger.record_approval_event(
                    owner,
                    &audit,
                    Phase::PolicyAllowed,
                    now(),
                    &value!({}),
                )?;
                true
            }
            Ok(ToolPolicy::AskOwner) => {
                let (mut review, decision) = pending_approval(proposal.clone());
                review.audit_id = Some(audit.clone());
                let verdict = if queue.offer(review) {
                    tokio::time::timeout(Duration::from_secs(300), decision)
                        .await
                        .ok()
                        .and_then(std::result::Result::ok)
                } else {
                    None
                };
                if verdict == Some(true) {
                    ledger.record_approval_event(
                        owner,
                        &audit,
                        Phase::OwnerAllowed,
                        now(),
                        &value!({"source":"host_queue"}),
                    )?;
                    if self.host.authorize(dispatch, execution).await {
                        ledger.approve_tool(owner, &proposal, now())?;
                        true
                    } else {
                        false
                    }
                } else {
                    ledger.record_approval_event(
                        owner,
                        &audit,
                        if verdict == Some(false) {
                            Phase::OwnerDenied
                        } else {
                            Phase::Expired
                        },
                        now(),
                        &value!({}),
                    )?;
                    false
                }
            }
            _ => {
                ledger.record_approval_event(
                    owner,
                    &audit,
                    Phase::PolicyDenied,
                    now(),
                    &value!({}),
                )?;
                false
            }
        };
        if !allowed || !self.host.authorize(dispatch, execution).await {
            if !ledger.approval_phase(owner, &audit)?.terminal() {
                ledger.record_approval_event(owner, &audit, Phase::Expired, now(), &value!({}))?;
            }
            return Ok(failure("send_file_denied"));
        }
        ledger.record_approval_event(owner, &audit, Phase::Applying, now(), &value!({}))?;
        pending.insert(audit.clone());
        let custody = match self.sender.begin(ledger, captured, &audit, now()) {
            Ok(c) => c,
            Err(_) => {
                ledger.record_approval_event(
                    owner,
                    &audit,
                    Phase::Failed,
                    now(),
                    &value!({"reason":"capture_or_local_authority_changed_before_io"}),
                )?;
                return Ok(failure("send_file_not_started"));
            }
        };
        let id = custody.record().id.clone();
        if let Err(reason) = self.send(ledger, owner, scope, &custody).await
            && !ledger.approval_phase(owner, &audit)?.terminal()
        {
            ledger.record_approval_event(
                owner,
                &audit,
                Phase::Unknown,
                now(),
                &value!({"reason":reason}),
            )?;
        }
        let record = ledger
            .sent_file(scope, dispatch, execution, &id)?
            .ok_or(crate::Error::Unknown)?;
        if !self.host.authorize(dispatch, execution).await
            || matches!(
                ledger.request_disposition(scope, dispatch)?,
                crate::RequestPolicy::Deny
            )
        {
            return Err(Error::Unknown);
        }
        Ok(output(
            &record,
            ledger.approval_phase(owner, &record.audit)?,
        ))
    }
    fn local(
        &self,
        ledger: &mut Ledger,
        scope: &Scope,
        id: &str,
        receipt: Delivery,
    ) -> crate::Result<Record> {
        ledger.observe_sent_file(scope, &self.dispatch, &self.execution, id, receipt)
    }
    async fn send(
        &self,
        ledger: &mut Ledger,
        owner: &str,
        scope: &Scope,
        custody: &Custody,
    ) -> std::result::Result<(), &'static str> {
        let mut record = custody.record().clone();
        let live = || self.host.authorize(&self.dispatch, &self.execution);
        if !live().await {
            return Err("current_authority_unavailable_before_prepare");
        }
        ledger
            .tool_disposition(custody.proposal(), now())
            .map_err(|_| "local_policy_changed_before_prepare")?;
        let prepared = self
            .host
            .prepare(&record.request_id, &record.manifest)
            .await
            .map_err(|_| "original_server_preparation_unconfirmed")?;
        record = self
            .local(ledger, scope, &record.id, prepared)
            .map_err(|_| "invalid_preparation_receipt")?;
        if record
            .remote
            .as_ref()
            .is_none_or(|r| r.state != "staging" || r.received_bytes != 0)
        {
            return Err("preparation_not_fresh");
        }
        for (index, bytes) in custody
            .bytes()
            .chunks(crate::file_delivery::CHUNK_BYTES)
            .enumerate()
        {
            if !live().await {
                return Err("current_authority_unavailable_before_chunk");
            }
            ledger
                .tool_disposition(custody.proposal(), now())
                .map_err(|_| "local_policy_changed_before_chunk")?;
            let receipt = self
                .host
                .chunk(
                    &record,
                    (index * crate::file_delivery::CHUNK_BYTES) as u64,
                    bytes,
                )
                .await
                .map_err(|_| "original_chunk_unconfirmed")?;
            record = self
                .local(ledger, scope, &record.id, receipt)
                .map_err(|_| "invalid_chunk_receipt")?;
        }
        if !live().await {
            return Err("current_authority_unavailable_before_queue");
        }
        ledger
            .tool_disposition(custody.proposal(), now())
            .map_err(|_| "local_policy_changed_before_queue")?;
        let queued = self
            .host
            .queue(&record)
            .await
            .map_err(|_| "original_queue_unconfirmed")?;
        record = self
            .local(ledger, scope, &record.id, queued)
            .map_err(|_| "invalid_queue_receipt")?;
        ledger.record_approval_event(owner,&record.audit,Phase::Submitted,now(),&value!({"deliveryId":record.id,"manifestDigest":record.manifest.digest().map_err(|_|"invalid_local_manifest")?})).map_err(|_|"local_submission_audit_unavailable")?;
        // Observe only the original operation for up to ten seconds. Never renew
        // authority, continue staged bytes, replace an attempt, or resend.
        for _ in 0..10 {
            if record
                .remote
                .as_ref()
                .is_some_and(|r| matches!(r.state.as_str(), "sent" | "unknown" | "cancelled"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            if !live().await {
                return Err("current_authority_unavailable_before_observation");
            }
            if matches!(
                ledger
                    .request_disposition(scope, &self.dispatch)
                    .map_err(|_| "local_request_policy_unavailable")?,
                crate::RequestPolicy::Deny
            ) {
                return Err("local_request_denied_before_observation");
            }
            let receipt = self
                .host
                .status(&record)
                .await
                .map_err(|_| "original_delivery_observation_unavailable")?;
            record = self
                .local(ledger, scope, &record.id, receipt)
                .map_err(|_| "invalid_delivery_observation")?;
        }
        self.audit(ledger, owner, &record)
            .map_err(|_| "local_delivery_audit_unavailable")?;
        Ok(())
    }
    fn audit(&self, ledger: &mut Ledger, owner: &str, record: &Record) -> crate::Result<()> {
        if ledger.approval_phase(owner, &record.audit)?.terminal() {
            return Ok(());
        }
        let Some(receipt) = &record.remote else {
            return Ok(());
        };
        let phase = match receipt.state.as_str() {
            "sent" => Phase::Completed,
            "unknown" => Phase::Unknown,
            "cancelled" => Phase::Failed,
            _ => return Ok(()),
        };
        ledger.record_approval_event(owner,&record.audit,phase,now(),&value!({"deliveryId":record.id,"state":receipt.state,"matrixEventId":receipt.matrix_event_id,"manifestDigest":receipt.manifest_digest}))?;
        Ok(())
    }
    async fn observe(
        &self,
        ledger: &mut Ledger,
        owner: &str,
        scope: &Scope,
        mut record: Record,
    ) -> Result<Value> {
        if record.remote.is_some()
            && let Ok(receipt) = self.host.status(&record).await
        {
            record = self.local(ledger, scope, &record.id, receipt)?;
            self.audit(ledger, owner, &record)?;
        }
        if !self.host.authorize(&self.dispatch, &self.execution).await
            || matches!(
                ledger.request_disposition(scope, &self.dispatch)?,
                crate::RequestPolicy::Deny
            )
        {
            return Err(Error::Unknown);
        }
        Ok(output(
            &record,
            ledger.approval_phase(owner, &record.audit)?,
        ))
    }
}
#[cfg(test)]
mod tests;
