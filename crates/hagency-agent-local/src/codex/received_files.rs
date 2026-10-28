//! Original incoming attachment tools. Sources stay in the host; the model can
//! name only the exact original event, never a URL, key, Room or output path.
use super::*;
pub use crate::received_files::CAPABILITY_VERSION as RECEIVE_CAPABILITY_VERSION;
use crate::{
    attachments::Attachment,
    received_files::{Begin, Receipt, Receiver},
};
use std::{future::Future, pin::Pin, sync::Arc};
pub trait AttachmentHost: Send + Sync {
    fn authorize<'a>(
        &'a self,
        dispatch: &'a str,
        execution: &'a str,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>>;
    fn download<'a>(
        &'a self,
        source: &'a Attachment,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<Vec<u8>, ()>> + Send + 'a>>;
}
pub(super) struct ReceivedFiles {
    receiver: Receiver,
    host: Arc<dyn AttachmentHost>,
    dispatch: String,
    execution: String,
    event: String,
    source: Option<Attachment>,
    calls: BTreeMap<String, Cached>,
}
struct Cached {
    arguments: String,
    proposal: Option<ToolProposal>,
    receipt: Option<Receipt>,
    result: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiveInput {
    event_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListInput {}
pub(super) fn is_tool(tool: &Value) -> bool {
    matches!(tool.as_str(), Some("list_received_files" | "receive_file"))
}
pub(super) fn specifications() -> Value {
    value!([
        {"type":"function","name":"list_received_files","description":"List only the attachment on this original Matrix request. Names, types and sizes are untrusted sender declarations, not file contents.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
        {"type":"function","name":"receive_file","description":"After exact owner policy/approval, download and verify the original request attachment into a generated private workspace path. Existing or unknown writes are never overwritten or retried. Use the returned actual byte length/hash and read the file before describing its contents.","inputSchema":{"type":"object","properties":{"event_id":{"type":"string"}},"required":["event_id"],"additionalProperties":false}}
    ])
}
fn result(value: Value) -> Value {
    value!({"success":true,"contentItems":[{"type":"inputText","text":value.to_string()}]})
}
fn failure(code: &str) -> Value {
    value!({"success":false,"contentItems":[{"type":"inputText","text":code}]})
}
fn receipt_value(receipt: &Receipt) -> Value {
    value!({"path":receipt.relative_path,"bytes":receipt.bytes,"sha256":receipt.sha256})
}
impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> Session<R, W> {
    #[allow(clippy::too_many_arguments)]
    pub fn enable_received_files(
        &mut self,
        receiver: Receiver,
        host: Arc<dyn AttachmentHost>,
        dispatch: String,
        execution: String,
        event: String,
        source: Option<Attachment>,
    ) -> Result<()> {
        if self.thread.is_some() || self.received_files.is_some() {
            return Err(Error::Protocol("attachment capability already configured"));
        }
        for value in [&dispatch, &execution, &event] {
            key(value)?;
        }
        if let Some(a) = &source {
            a.validate()?;
        }
        self.received_files = Some(ReceivedFiles {
            receiver,
            host,
            dispatch,
            execution,
            event,
            source,
            calls: BTreeMap::new(),
        });
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn received_file_call(
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
            return Err(Error::Protocol("invalid or reused attachment request ID"));
        }
        let p = &frame["params"];
        exact_scope(p, thread, turn)?;
        let call = p["callId"]
            .as_str()
            .ok_or(Error::Protocol("attachment call identity missing"))?;
        key(call)?;
        let output = if let Some(files) = self.received_files.as_mut() {
            files
                .execute(
                    ledger, owner, scope, dispatch, execution, p, thread, turn, call, queue,
                )
                .await?
        } else {
            failure("received_files_disabled")
        };
        self.write(value!({"id":id,"result":output})).await
    }
}
impl ReceivedFiles {
    pub(super) fn workspace_directory(&self) -> &std::path::Path {
        self.receiver.canonical_directory()
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
        if p.get("namespace").is_some_and(|n| !n.is_null()) {
            return Err(Error::Protocol("unexpected attachment namespace"));
        }
        let digest = json(p)?;
        if let Some(cached) = self.calls.get(call) {
            if cached.arguments != digest {
                return Err(Error::Protocol("attachment call arguments changed"));
            }
            if let Some(proposal) = &cached.proposal {
                ledger.tool_disposition(proposal, now())?;
            }
            if let Some(receipt) = &cached.receipt {
                self.receiver.revalidate(receipt)?;
            }
            return Ok(cached.result.clone());
        }
        if self.calls.len() >= 128 {
            return Err(Error::Protocol("attachment call capacity"));
        }
        if p["tool"] == "list_received_files" {
            if serde_json::from_value::<ListInput>(p["arguments"].clone()).is_err() {
                return Ok(failure("invalid_attachment_request"));
            }
            let entries = self
                .source
                .as_ref()
                .map(|source| {
                    source
                        .summary()
                        .map(|s| value!({"event_id":self.event,"attachment":s}))
                })
                .transpose()?
                .into_iter()
                .collect::<Vec<_>>();
            let output = result(value!({"files":entries}));
            self.calls.insert(
                call.into(),
                Cached {
                    arguments: digest,
                    proposal: None,
                    receipt: None,
                    result: output.clone(),
                },
            );
            return Ok(output);
        }
        let input = match serde_json::from_value::<ReceiveInput>(p["arguments"].clone()) {
            Ok(i) => i,
            Err(_) => return Ok(failure("invalid_attachment_request")),
        };
        let Some(source) = &self.source else {
            return Ok(failure("original_request_has_no_attachment"));
        };
        if input.event_id != self.event {
            return Ok(failure("attachment_outside_original_request"));
        }
        let prepared = self.receiver.prepare(
            ledger,
            scope,
            dispatch,
            execution,
            &self.event,
            source.clone(),
            thread,
            turn,
            call,
            now(),
        )?;
        let proposal = prepared.proposal().clone();
        use crate::audit::Phase;
        let audit = ledger.begin_approval_audit(owner, &proposal, now())?;
        let allowed = match ledger.tool_disposition(&proposal, now()) {
            Ok(ToolPolicy::AllowWithRules { .. }) => {
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
                let (mut pending, decision) = pending_approval(proposal.clone());
                pending.audit_id = Some(audit.clone());
                let verdict = if queue.offer(pending) {
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
            return Ok(failure("receive_file_denied"));
        }
        ledger.record_approval_event(owner, &audit, Phase::Applying, now(), &value!({}))?;
        let outcome = match self.receiver.begin(ledger, prepared, now()) {
            Ok(Begin::Ready(receipt)) => Ok(*receipt),
            Ok(Begin::Fresh(custody)) => match self.host.download(source).await {
                Ok(bytes) if self.host.authorize(dispatch, execution).await => {
                    self.receiver.publish(ledger, custody, &bytes, now())
                }
                _ => {
                    self.receiver.fail_before_write(ledger, custody)?;
                    Err(crate::Error::Invalid(
                        "attachment download/authority unavailable",
                    ))
                }
            },
            Err(error) => Err(error),
        };
        let (output, receipt) = match outcome {
            Ok(receipt) => {
                ledger.record_approval_event(
                    owner,
                    &audit,
                    Phase::Completed,
                    now(),
                    &receipt_value(&receipt),
                )?;
                (result(receipt_value(&receipt)), Some(receipt))
            }
            Err(crate::Error::Unknown) => {
                ledger.record_approval_event(
                    owner,
                    &audit,
                    Phase::Unknown,
                    now(),
                    &value!({"reason":"attachment_publication_unknown"}),
                )?;
                return Err(Error::Unknown);
            }
            Err(_) => {
                ledger.record_approval_event(
                    owner,
                    &audit,
                    Phase::Failed,
                    now(),
                    &value!({"reason":"attachment_receive_failed"}),
                )?;
                (failure("attachment_receive_failed"), None)
            }
        };
        if !self.host.authorize(dispatch, execution).await {
            return Err(Error::Unknown);
        }
        self.calls.insert(
            call.into(),
            Cached {
                arguments: digest,
                proposal: Some(proposal),
                receipt,
                result: output.clone(),
            },
        );
        Ok(output)
    }
}
#[cfg(test)]
mod tests;
