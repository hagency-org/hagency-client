//! Exact recipient identities. A saved observation is never a runtime grant.
use super::canonical::exact_id;
use hagency_store::Error;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum OperatorGraphRecipientTarget {
    SharedAgent {
        project_id: String,
        agent_request_id: String,
    },
    PersonalAgent {
        agent_id: String,
        binding_id: String,
    },
    Human {
        project_id: String,
        room_id: String,
        user_id: String,
    },
}
pub(crate) fn key(value: &str) -> Result<(), Error> {
    hagency_core::project::identifier(value, 128)?;
    if !value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(hagency_core::InvalidInput("invalid recipient identifier").into());
    }
    Ok(())
}
pub(crate) fn matrix_id(value: &str, prefix: char) -> Result<(), Error> {
    exact_id(value)?;
    if !value.starts_with(prefix)
        || !value[1..]
            .split_once(':')
            .is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
        || value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '/' | '\\' | '?' | '#' | '%'))
    {
        return Err(hagency_core::InvalidInput("invalid Matrix identifier").into());
    }
    Ok(())
}
impl OperatorGraphRecipientTarget {
    pub(crate) fn validate(&self) -> Result<(), Error> {
        match self {
            Self::SharedAgent {
                project_id,
                agent_request_id,
            } => {
                key(project_id)?;
                key(agent_request_id)
            }
            Self::PersonalAgent {
                agent_id,
                binding_id,
            } => {
                key(agent_id)?;
                key(binding_id)
            }
            Self::Human {
                project_id,
                room_id,
                user_id,
            } => {
                key(project_id)?;
                matrix_id(room_id, '!')?;
                matrix_id(user_id, '@')
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorGraphRecipientSnapshot {
    pub graph_id: String,
    pub node_id: String,
    pub revision: u64,
    pub target: OperatorGraphRecipientTarget,
    pub account: String,
    pub server: String,
    pub recipient_mxid: String,
    pub room_id: String,
    /// Exact native serialization avoids the mini-app VM's JSON depth/number limits.
    pub identity_json: String,
    pub observation_digest: String,
    pub runtime_delivery_authorized: bool,
}
impl OperatorGraphRecipientSnapshot {
    pub(crate) fn digest(&self) -> Result<String, Error> {
        Ok(hagency_core::project::hash(&serde_json::to_vec(&json!({
            "protocol":"hagency.graph.recipient.v1", "graphId":self.graph_id,
            "nodeId":self.node_id,"revision":self.revision,"target":self.target,
            "account":self.account,"server":self.server,"recipientMxid":self.recipient_mxid,
            "roomId":self.room_id,"identityJson":self.identity_json
        }))?))
    }
    pub(crate) fn validate(&self) -> Result<(), Error> {
        exact_id(&self.graph_id)?;
        exact_id(&self.node_id)?;
        self.target.validate()?;
        matrix_id(&self.account, '@')?;
        matrix_id(&self.recipient_mxid, '@')?;
        matrix_id(&self.room_id, '!')?;
        if self.revision == 0
            || self.revision >= i64::MAX as u64
            || self.server.len() > 2048
            || self.runtime_delivery_authorized
            || serde_json::to_vec(self)?.len() > 32 * 1024
            || self.observation_digest != self.digest()?
        {
            return Err(hagency_core::InvalidInput("invalid recipient snapshot").into());
        }
        let identity: Value = serde_json::from_str(&self.identity_json)?;
        if !identity.is_object() {
            return Err(hagency_core::InvalidInput("invalid recipient identity").into());
        }
        Ok(())
    }
}

/// Only identity/send-relevant events enter equality. Presence, typing, read
/// receipts, observation timestamps and unrelated room events cannot churn it.
pub(crate) fn matrix_proof(
    state: &Value,
    owner: &str,
    recipient: &str,
) -> Result<(Value, Vec<String>), Error> {
    let invalid = || Error::from(hagency_core::InvalidInput("invalid current room state"));
    let events = state
        .as_array()
        .filter(|a| a.len() <= 16384)
        .ok_or_else(invalid)?;
    let get = |kind: &str, key: &str| -> Result<Option<&Value>, Error> {
        let mut found = events
            .iter()
            .filter(|e| e["type"] == kind && e["state_key"] == key);
        let event = found.next();
        if found.next().is_some() || event.is_some_and(|e| !e["content"].is_object()) {
            return Err(invalid());
        }
        Ok(event)
    };
    let project = |event: Option<&Value>| -> Value {
        event.map_or(Value::Null, |e| json!({"type":e["type"],"stateKey":e["state_key"],"eventId":e["event_id"],"sender":e["sender"],"content":e["content"]}))
    };
    let owner_event = get("m.room.member", owner)?;
    let recipient_event = get("m.room.member", recipient)?;
    let create = get("m.room.create", "")?;
    let power = get("m.room.power_levels", "")?;
    let encryption = get("m.room.encryption", "")?;
    let mut blockers = Vec::new();
    if owner_event.is_none_or(|e| e["content"]["membership"] != "join") {
        blockers.push("sender_not_joined".into());
    }
    if recipient_event.is_none_or(|e| e["content"]["membership"] != "join") {
        blockers.push("recipient_not_joined".into());
    }
    if create.is_none() {
        blockers.push("room_identity_unavailable".into());
    }
    let event_type = if encryption.is_some() {
        "m.room.encrypted"
    } else {
        "m.room.message"
    };
    let integer = |v: &Value, default: i64| -> Result<i64, Error> {
        if v.is_null() {
            Ok(default)
        } else {
            v.as_i64().ok_or_else(invalid)
        }
    };
    let (sender_power, required) = if let Some(power) = power {
        let p = &power["content"];
        (
            integer(&p["users"][owner], integer(&p["users_default"], 0)?)?,
            integer(&p["events"][event_type], integer(&p["events_default"], 0)?)?,
        )
    } else {
        (
            if create.is_some_and(|e| e["content"]["creator"] == owner || e["sender"] == owner) {
                100
            } else {
                0
            },
            0,
        )
    };
    if sender_power < required {
        blockers.push("sender_cannot_send".into());
    }
    Ok((
        json!({"senderMembership":project(owner_event),"recipientMembership":project(recipient_event),"create":project(create),"powerLevels":project(power),"encryption":project(encryption),"eventType":event_type,"senderPower":sender_power,"requiredPower":required}),
        blockers,
    ))
}
