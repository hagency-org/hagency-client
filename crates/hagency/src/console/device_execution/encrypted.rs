//! Data-only encrypted admission protocol; SDK proof and durable local commit stay host-only.
use super::*;
use std::collections::BTreeSet;
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Projection {
    pub thread_root: Option<String>,
    pub mentioned_mxids: BTreeSet<String>,
}
impl Projection {
    pub fn from_message(message: &Value) -> Result<Self, DeviceError> {
        let c = &message["content"];
        if message["type"] != "m.room.message"
            || !matches!(
                c["msgtype"].as_str(),
                Some("m.text" | "m.file" | "m.image" | "m.audio" | "m.video")
            )
            || c["body"]
                .as_str()
                .is_none_or(|s| s.is_empty() || s.len() > 65536)
            || c["m.relates_to"]["rel_type"] == "m.replace"
        {
            return Err(invalid());
        }
        let root = if c["m.relates_to"]["rel_type"] == "m.thread" {
            Some(
                c["m.relates_to"]["event_id"]
                    .as_str()
                    .ok_or_else(invalid)?
                    .into(),
            )
        } else {
            None
        };
        let mentioned_mxids = c["m.mentions"]["user_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let result = Self {
            thread_root: root,
            mentioned_mxids,
        };
        result.validate()?;
        Ok(result)
    }
    pub(super) fn validate(&self) -> Result<(), DeviceError> {
        if self.mentioned_mxids.len() > 128
            || self.thread_root.as_ref().is_some_and(|s| {
                !s.starts_with('$')
                    || s.len() < 2
                    || s.len() > 512
                    || s.chars().any(|c| c.is_control() || c.is_whitespace())
            })
            || self.mentioned_mxids.iter().any(|s| {
                !s.starts_with('@')
                    || !s.contains(':')
                    || s.len() > 512
                    || s.chars().any(|c| c.is_control() || c.is_whitespace())
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Admission {
    pub inbox_id: String,
    pub source_digest: String,
    pub agent_generation: i64,
    pub owner_direct: bool,
    pub dispatch: Dispatch,
}
fn invalid() -> DeviceError {
    error(502, "invalid_encrypted_admission")
}
impl Admission {
    pub(super) fn check(
        &self,
        source: &Ciphertext,
        lease: &LeaseRef,
        device: &AuthorizedDevice,
        projection: Option<&Projection>,
    ) -> Result<(), DeviceError> {
        source.validate(lease)?;
        let e = &self.dispatch;
        id(&e.id).map_err(|_| invalid())?;
        let body = hagency_agent_local::encrypted_inputs::canonical(&source.event)
            .map_err(|_| invalid())?;
        if self.inbox_id != source.id
            || self.source_digest != source.source_digest
            || self.agent_generation != source.agent_generation
            || e.binding_id != source.binding_id
            || e.agent_id != source.agent_id
            || e.event_id != source.event_id
            || e.room_id != source.room_id
            || e.requester_mxid != source.sender_mxid
            || e.binding_generation != source.binding_generation
            || e.body != body
            || !matches!(
                e.state.as_str(),
                "encrypted_offered" | "encrypted_acknowledged"
            )
            || e.dispatch_epoch != Some(lease.epoch)
            || e.dispatch_device_id.as_deref() != Some(device.device_id())
            || e.execution_id.is_some()
            || e.outcome.is_some()
        {
            return Err(invalid());
        }
        if let Some(p) = projection {
            let root = p.thread_root.as_deref().unwrap_or(if self.owner_direct {
                &source.room_id
            } else {
                &source.event_id
            });
            if e.thread_root != root {
                return Err(invalid());
            }
        }
        Ok(())
    }
    pub fn local_dispatch(
        &self,
        message: &Value,
    ) -> Result<hagency_agent_local::inbox::Dispatch, DeviceError> {
        let mut value = serde_json::to_value(&self.dispatch).map_err(|_| invalid())?;
        value["body"] = message["content"]["body"].clone();
        value["state"] = json!(match self.dispatch.state.as_str() {
            "encrypted_offered" => "offered",
            "encrypted_acknowledged" => "acknowledged",
            _ => return Err(invalid()),
        });
        serde_json::from_value(value).map_err(|_| invalid())
    }
    pub(super) fn same_input(&self, prior: &Self) -> bool {
        let a = &self.dispatch;
        let b = &prior.dispatch;
        self.inbox_id == prior.inbox_id
            && self.source_digest == prior.source_digest
            && self.agent_generation == prior.agent_generation
            && self.owner_direct == prior.owner_direct
            && a.id == b.id
            && a.binding_id == b.binding_id
            && a.agent_id == b.agent_id
            && a.event_id == b.event_id
            && a.room_id == b.room_id
            && a.requester_mxid == b.requester_mxid
            && a.thread_root == b.thread_root
            && a.body == b.body
            && a.binding_generation == b.binding_generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Ciphertext, Admission, LeaseRef, AuthorizedDevice) {
        let source: Ciphertext =
            serde_json::from_str(include_str!("ciphertext-wire-fixture.json")).unwrap();
        let lease = LeaseRef {
            agent_id: source.agent_id.clone(),
            epoch: 1,
        };
        let device = crate::console::server_login::fixture_device(
            "http://127.0.0.1:9/",
            "owner",
            "@owner:test",
        )
        .0;
        let dispatch = Dispatch {
            id: "evt_original".into(),
            binding_id: source.binding_id.clone(),
            agent_id: source.agent_id.clone(),
            event_id: source.event_id.clone(),
            room_id: source.room_id.clone(),
            requester_mxid: source.sender_mxid.clone(),
            thread_root: source.room_id.clone(),
            body: hagency_agent_local::encrypted_inputs::canonical(&source.event).unwrap(),
            state: "encrypted_offered".into(),
            binding_generation: source.binding_generation,
            dispatch_epoch: Some(1),
            dispatch_device_id: Some(device.device_id().into()),
            execution_id: None,
            outcome: None,
        };
        let a = Admission {
            inbox_id: source.id.clone(),
            source_digest: source.source_digest.clone(),
            agent_generation: source.agent_generation,
            owner_direct: true,
            dispatch,
        };
        (source, a, lease, device)
    }
    #[test]
    fn encrypted_admission_wire_never_sends_plaintext_and_cannot_substitute_source_or_mode() {
        let (source, a, lease, device) = fixture();
        let message = json!({"type":"m.room.message","content":{"msgtype":"m.text","body":"private-input-never-on-server"}});
        let projection = Projection::from_message(&message).unwrap();
        let op = DeviceOperation::PrepareCiphertext {
            lease: lease.clone(),
            expected: source.clone(),
            projection,
        };
        let (path, wire) = op.request().unwrap();
        assert_eq!(path, "ciphertexts/prepare");
        assert!(
            !String::from_utf8(wire)
                .unwrap()
                .contains("private-input-never-on-server")
        );
        parse_response(&op, json!({"admission":a}), &device).unwrap();
        for (field, value) in [
            ("sourceDigest", json!("changed")),
            ("agentGeneration", json!(0)),
            ("inboxId", json!("enc_other")),
        ] {
            let mut wrong = serde_json::to_value(&a).unwrap();
            wrong[field] = value;
            assert!(parse_response(&op, json!({"admission":wrong}), &device).is_err());
        }
        for (field, value) in [
            ("body", json!("private-input-never-on-server")),
            ("bindingId", json!("bnd_other")),
            ("state", json!("offered")),
            ("threadRoot", json!("$outer-injected")),
        ] {
            let mut wrong = serde_json::to_value(&a).unwrap();
            wrong["dispatch"][field] = value;
            assert!(parse_response(&op, json!({"admission":wrong}), &device).is_err());
        }
        let mut wrong = serde_json::to_value(&a).unwrap();
        wrong["verified"] = json!(true);
        assert!(serde_json::from_value::<Admission>(wrong).is_err());
        let ack = DeviceOperation::AckCiphertext {
            lease,
            expected: source,
            prepared: Box::new(a.clone()),
        };
        let mut accepted = a.clone();
        accepted.dispatch.state = "encrypted_acknowledged".into();
        parse_response(&ack, json!({"admission":accepted}), &device).unwrap();
        accepted.dispatch.id = "evt_replacement".into();
        assert!(parse_response(&ack, json!({"admission":accepted}), &device).is_err());
    }
    #[tokio::test]
    async fn encrypted_device_retirement_fence_rejects_before_authority_or_http_setup() {
        let root = tempfile::tempdir().unwrap();
        let owner = crate::console::native::NativeOwner::open(
            &root.path().join("state"),
            "http://127.0.0.1:9",
            "@owner:test",
        )
        .await
        .unwrap();
        let (source, _, lease, device) = fixture();
        let op = DeviceOperation::CiphertextAdmission {
            lease,
            expected: source,
        };
        let error = owner
            .test_console()
            .execution_api_pinned_live(
                op,
                &DeviceIdentity::from(&device),
                std::sync::Arc::new(|| false),
            )
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, "agent_crypto_scope_retired");
        owner.shutdown_checked().await.unwrap();
    }
}
