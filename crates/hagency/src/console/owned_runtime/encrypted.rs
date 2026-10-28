//! Current runtime adapter for original encrypted owner input and ciphertext output.
use super::*;
use crate::console::device_execution::{Dispatch, ExecutionAttachments, ExecutionStart};
use crate::console::native::{AgentCryptoTransport, NativeOwner};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
pub(super) fn file_receipt(
    record: &hagency_agent_local::sent_files::Record,
    publication: &Value,
) -> Result<hagency_agent_local::file_delivery::Delivery, RuntimeError> {
    let mut receipt = record.remote.clone().ok_or(RuntimeError::Transport)?;
    if !receipt.id.starts_with("efile_")
        || publication["dispatchId"] != receipt.dispatch_id
        || publication["executionId"] != receipt.execution_id
        || publication["requestId"] != record.request_id
        || publication["kind"] != "file"
        || !matches!(publication["state"].as_str(), Some("possible" | "sent"))
    {
        return Err(RuntimeError::Transport);
    }
    receipt.received_bytes = record.manifest.bytes;
    receipt.upload_accepted = true;
    receipt.state = if publication["state"] == "sent" {
        "sent"
    } else {
        "unknown"
    }
    .into();
    receipt.matrix_event_id = publication["matrixEventId"].as_str().map(str::to_owned);
    receipt
        .validate(
            &receipt.dispatch_id,
            &receipt.execution_id,
            &record.request_id,
            &record.manifest,
        )
        .map_err(|_| RuntimeError::Transport)?;
    Ok(receipt)
}
pub(super) fn native(host: &PinnedConsole) -> Option<NativeOwner> {
    host.console
        .0
        .native_owner
        .get()?
        .upgrade()
        .map(NativeOwner)
}
pub(super) async fn ledger(host: &PinnedConsole) -> Result<(Ledger, String), RuntimeError> {
    let d = host
        .console
        .authorized_device()
        .await
        .map_err(device_error)?;
    if DevicePin::from(&d) != host.pin {
        return Err(RuntimeError::Authorization);
    }
    let root = host
        .console
        .0
        .server_login
        .state_directory()
        .ok_or(RuntimeError::Profile)?;
    let path = ledger_path(&root, d.origin(), d.issuer(), d.subject(), d.owner_mxid())
        .map_err(|_| RuntimeError::Profile)?;
    let profile = profile_identity(d.origin(), d.issuer(), d.subject(), d.owner_mxid())
        .map_err(|_| RuntimeError::Profile)?;
    Ok((
        local(Ledger::open_scoped(&path, d.owner_mxid(), &profile))?,
        d.owner_mxid().to_owned(),
    ))
}
pub(super) async fn crypto(
    host: &PinnedConsole,
    agent: &str,
) -> Result<Arc<dyn AgentCryptoTransport>, RuntimeError> {
    native(host)
        .ok_or(RuntimeError::Authorization)?
        .runtime_agent_crypto(agent)
        .await
        .map_err(|_| RuntimeError::Authorization)
}
pub(super) async fn call(
    host: &PinnedConsole,
    lease: &LeaseRef,
    dispatch: &str,
    execution: &str,
    action: &str,
    extra: Value,
) -> Result<Value, RuntimeError> {
    let d = host
        .console
        .authorized_device()
        .await
        .map_err(device_error)?;
    if DevicePin::from(&d) != host.pin {
        return Err(RuntimeError::Authorization);
    }
    let mut body =
        json!({"lease":lease,"dispatchId":dispatch,"executionId":execution,"action":action});
    for (k, v) in extra.as_object().ok_or(RuntimeError::Transport)? {
        body[k] = v.clone();
    }
    let bytes = serde_json::to_vec(&body).map_err(|_| RuntimeError::Profile)?;
    if bytes.len() > 131072 {
        return Err(RuntimeError::Profile);
    }
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| RuntimeError::Transport)?;
    let mut response = http
        .post(format!(
            "{}hagency/api/v1/execution/encrypted/action",
            host.pin.origin
        ))
        .bearer_auth(d.bearer().map_err(|_| RuntimeError::Authorization)?)
        .header("content-type", "application/json")
        .body(bytes)
        .send()
        .await
        .map_err(|_| RuntimeError::Transport)?;
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| RuntimeError::Transport)?
    {
        if bytes.len() + chunk.len() > 131072 {
            return Err(RuntimeError::Transport);
        }
        bytes.extend(chunk);
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| RuntimeError::Transport)?;
    let after = host
        .console
        .authorized_device()
        .await
        .map_err(device_error)?;
    if DevicePin::from(&after) != host.pin {
        return Err(RuntimeError::Authorization);
    }
    if !(200..300).contains(&status) {
        return Err(transport_error(
            status,
            value["error"]
                .as_str()
                .unwrap_or("encrypted_request_refused"),
            false,
            true,
        ));
    }
    Ok(value)
}
fn rebind(
    ledger: &Ledger,
    owner: &str,
    value: &Value,
    expected: &inbox::Dispatch,
    lease: &LeaseRef,
) -> Result<Dispatch, RuntimeError> {
    let mut d: Dispatch =
        serde_json::from_value(value.clone()).map_err(|_| RuntimeError::Transport)?;
    let tuple = json!([
        d.id,
        d.binding_id,
        d.agent_id,
        d.event_id,
        d.room_id,
        d.requester_mxid,
        d.thread_root,
        d.body,
        d.binding_generation
    ]);
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&tuple).map_err(|_| RuntimeError::Transport)?)
    );
    if local(ledger.encrypted_input_transport_digest(owner, &d.id))?.as_deref() != Some(&digest)
        || d.id != expected.id
        || d.dispatch_epoch != Some(lease.epoch)
        || d.dispatch_device_id != expected.dispatch_device_id
        || !d.state.starts_with("encrypted_")
    {
        return Err(RuntimeError::Transport);
    }
    d.body = expected.body.clone();
    d.state = d.state.trim_start_matches("encrypted_").into();
    Ok(d)
}
pub(super) async fn execute(
    host: &PinnedConsole,
    op: &Op,
) -> Result<Option<Response>, RuntimeError> {
    if let Op::Poll {
        lease, binding_id, ..
    }
    | Op::PollFiles {
        lease, binding_id, ..
    } = op
    {
        let Some(native) = native(host) else {
            return Ok(None);
        };
        // Ordinary requests continue through their existing source and routing checks.
        let plain = host
            .console
            .execution_api_pinned(
                match op {
                    Op::PollFiles { limit, .. } => Op::PollFiles {
                        lease: lease.clone(),
                        binding_id: binding_id.clone(),
                        limit: *limit,
                    },
                    Op::Poll { limit, .. } => Op::Poll {
                        lease: lease.clone(),
                        binding_id: binding_id.clone(),
                        limit: *limit,
                    },
                    _ => unreachable!(),
                },
                &host.pin,
            )
            .await
            .map_err(|e| transport_error(e.status, &e.code, false, true))?;
        if let Response::Events(events) = plain
            && !events.is_empty()
        {
            return Ok(Some(Response::Events(events)));
        }
        if native.runtime_agent_crypto(&lease.agent_id).await.is_err() {
            return Ok(Some(Response::Events(vec![])));
        }
        let sources = native
            .poll_agent_ciphertexts(lease, binding_id, 4)
            .await
            .map_err(|e| transport_error(e.status, &e.code, false, true))?;
        let mut events = Vec::new();
        for source in sources {
            let receipt = match native.receive_agent_ciphertext(lease, &source.id).await {
                Ok(r) => r,
                Err(e)
                    if matches!(
                        e.code.as_str(),
                        "agent_crypto_unauthenticated_relation"
                            | "encrypted_ciphertext_replayed"
                            | "agent_crypto_event_not_decryptable"
                    ) =>
                {
                    continue;
                }
                Err(e) => return Err(transport_error(e.status, &e.code, false, true)),
            };
            let id = receipt["dispatchId"]
                .as_str()
                .ok_or(RuntimeError::Transport)?;
            let (l, owner) = ledger(host).await?;
            let record = local(l.inbox_record(&owner, id))?;
            let wire = serde_json::from_value(
                serde_json::to_value(record.dispatch).map_err(|_| RuntimeError::Profile)?,
            )
            .map_err(|_| RuntimeError::Profile)?;
            events.push(wire);
        }
        return Ok(Some(Response::Events(events)));
    }
    let (lease, id, execution, action) = match op {
        Op::Start {
            lease,
            dispatch_id,
            execution_id,
        } => (lease, dispatch_id, execution_id.as_str(), "start"),
        Op::AuthorizeTool {
            lease,
            dispatch_id,
            execution_id,
        } => (lease, dispatch_id, execution_id.as_str(), "authorize"),
        Op::Attachments {
            lease,
            dispatch_id,
            execution_id,
        } => (lease, dispatch_id, execution_id.as_str(), "attachments"),
        Op::Ack { lease, dispatch_id } => (lease, dispatch_id, "", "ack"),
        Op::Processing {
            lease,
            dispatch_id,
            execution_id,
        } => (lease, dispatch_id, execution_id.as_str(), "processing"),
        Op::Finish {
            lease,
            dispatch_id,
            execution_id,
            ..
        } => (lease, dispatch_id, execution_id.as_str(), "finish"),
        _ => return Ok(None),
    };
    if native(host).is_none() {
        return Ok(None);
    }
    let (l, owner) = ledger(host).await?;
    if local(l.encrypted_input_transport_digest(&owner, id))?.is_none() {
        return Ok(None);
    }
    let input = local(l.encrypted_input(&owner, id))?;
    let record = local(l.inbox_record(&owner, id))?;
    if action == "ack" {
        return Ok(Some(Response::Acknowledged));
    }
    if action == "processing" {
        return Err(RuntimeError::Request);
    }
    let response = call(
        host,
        lease,
        id,
        execution,
        if action == "attachments" {
            "authorize"
        } else {
            action
        },
        json!({}),
    )
    .await?;
    if matches!(action, "authorize" | "attachments") {
        let d = rebind(&l, &owner, &response["dispatch"], &record.dispatch, lease)?;
        if d.state != "running" || d.execution_id.as_deref() != Some(execution) {
            return Err(RuntimeError::Transport);
        }
    }
    Ok(Some(match action {
        "start" => Response::Started(ExecutionStart {
            dispatch: rebind(&l, &owner, &response["dispatch"], &record.dispatch, lease)?,
            newly_started: response["newlyStarted"]
                .as_bool()
                .ok_or(RuntimeError::Transport)?,
        }),
        "authorize" => Response::ToolAuthorized(rebind(
            &l,
            &owner,
            &response["dispatch"],
            &record.dispatch,
            lease,
        )?),
        "attachments" => Response::Attachments(ExecutionAttachments {
            dispatch_id: id.clone(),
            execution_id: execution.into(),
            attachments: if input.as_ref().ok_or(RuntimeError::Profile)?.message["content"]["msgtype"]
                == "m.text"
            {
                vec![]
            } else {
                vec![local(
                    hagency_agent_local::attachments::Attachment::from_content(
                        &input.as_ref().ok_or(RuntimeError::Profile)?.message["content"],
                    ),
                )?]
            },
        }),
        "finish" => Response::Finished,
        _ => return Err(RuntimeError::Transport),
    }))
}
pub(super) async fn publish(
    host: &PinnedConsole,
    lease: &LeaseRef,
    dispatch: &inbox::Dispatch,
    execution: &str,
    kind: &str,
    request: &str,
    content: Option<Value>,
) -> Result<Value, RuntimeError> {
    let (mut ledger, owner) = ledger(host).await?;
    let original = local(ledger.encrypted_output(&owner, &dispatch.id, kind, request))?;
    let payload = if let Some(original) = original {
        original
    } else {
        let content = content.ok_or(RuntimeError::Request)?;
        let auth = call(host, lease, &dispatch.id, execution, "authorize", json!({})).await?;
        let authorized = rebind(&ledger, &owner, &auth["dispatch"], dispatch, lease)?;
        if authorized.state != "running" || authorized.execution_id.as_deref() != Some(execution) {
            return Err(RuntimeError::Transport);
        }
        let payload = crypto(host, &dispatch.agent_id)
            .await?
            .seal_reply(&dispatch.room_id, content)
            .await
            .map_err(|_| RuntimeError::Transport)?;
        local(ledger.retain_encrypted_output(&owner, &dispatch.id, kind, request, &payload))?;
        payload
    };
    let result = call(
        host,
        lease,
        &dispatch.id,
        execution,
        if dispatch.dispatch_epoch == Some(lease.epoch) {
            "publish"
        } else {
            "status"
        },
        if dispatch.dispatch_epoch == Some(lease.epoch) {
            json!({"kind":kind,"requestId":request,"payload":payload})
        } else {
            json!({"kind":kind,"requestId":request})
        },
    )
    .await?;
    let p = &result["publication"];
    if p["dispatchId"] != dispatch.id
        || p["executionId"] != execution
        || p["requestId"] != request
        || p["kind"] != kind
        || !matches!(p["state"].as_str(), Some("possible" | "sent"))
        || p["state"] == "sent"
            && p["matrixEventId"]
                .as_str()
                .is_none_or(|s| !s.starts_with('$'))
    {
        return Err(RuntimeError::Transport);
    }
    Ok(p.clone())
}
pub(super) fn relation(content: &mut Value, dispatch: &inbox::Dispatch) {
    if dispatch.thread_root != dispatch.room_id {
        content["m.relates_to"] = json!({"rel_type":"m.thread","event_id":dispatch.thread_root,"is_falling_back":true,"m.in_reply_to":{"event_id":dispatch.event_id}});
    }
}
pub(super) async fn reply(
    host: &PinnedConsole,
    lease: &LeaseRef,
    record: &inbox::Record,
) -> Result<Option<String>, RuntimeError> {
    let mut content =
        json!({"msgtype":"m.text","body":record.reply.as_ref().ok_or(RuntimeError::Profile)?});
    relation(&mut content, &record.dispatch);
    let p = publish(
        host,
        lease,
        &record.dispatch,
        record
            .execution_id
            .as_deref()
            .ok_or(RuntimeError::Profile)?,
        "reply",
        "reply",
        Some(content),
    )
    .await?;
    Ok(if p["state"] == "sent" {
        Some(p["matrixEventId"].as_str().unwrap().into())
    } else {
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encrypted_runtime_original_cipher_commitment_rejects_plaintext_scope_or_epoch_substitution()
    {
        let owner = "@owner:test";
        let mut l = Ledger::open_scoped(":memory:", owner, &"a".repeat(64)).unwrap();
        let d = inbox::Dispatch {
            id: "evt_original".into(),
            binding_id: "bnd_original".into(),
            agent_id: "agt_original".into(),
            event_id: "$original".into(),
            room_id: "!private:test".into(),
            requester_mxid: owner.into(),
            thread_root: "!private:test".into(),
            body: "private original".into(),
            state: "offered".into(),
            binding_generation: 1,
            dispatch_epoch: Some(1),
            dispatch_device_id: Some("dev_original".into()),
            execution_id: None,
            outcome: None,
        };
        l.register_binding(owner, &d.scope()).unwrap();
        let c = json!({"type":"m.room.encrypted","event_id":d.event_id,"room_id":d.room_id,"sender":owner,"origin_server_ts":1,"content":{"algorithm":"m.megolm.v1.aes-sha2","ciphertext":"c2VhbGVk","session_id":"session"}});
        let input = hagency_agent_local::encrypted_inputs::Input {
            inbox_id: "enc_original".into(),
            source_digest: crate::console::device_execution::ciphertexts::source_digest(&c)
                .unwrap(),
            agent_generation: 1,
            owner_direct: true,
            ciphertext: c.clone(),
            message: json!({"type":"m.room.message","event_id":d.event_id,"sender":owner,"content":{"msgtype":"m.text","body":d.body}}),
        };
        l.receive_encrypted_dispatch(owner, &d, &input, inbox::Limits::default(), 10)
            .unwrap();
        let mut wire = serde_json::to_value(&d).unwrap();
        wire["body"] = json!(hagency_agent_local::encrypted_inputs::canonical(&c).unwrap());
        wire["state"] = json!("encrypted_running");
        wire["executionId"] = json!("exec_original");
        let lease = LeaseRef {
            agent_id: d.agent_id.clone(),
            epoch: 1,
        };
        assert_eq!(rebind(&l, owner, &wire, &d, &lease).unwrap().body, d.body);
        for (k, v) in [
            ("body", json!(d.body)),
            ("roomId", json!("!foreign:test")),
            ("dispatchEpoch", json!(2)),
            ("dispatchDeviceId", json!("dev_other")),
            ("state", json!("running")),
        ] {
            let mut changed = wire.clone();
            changed[k] = v;
            assert!(rebind(&l, owner, &changed, &d, &lease).is_err());
        }
    }
}
