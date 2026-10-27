//! Untrusted wire data until response/context validation; never execution grants.
use super::*;
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ciphertext {
    pub id: String,
    pub binding_id: String,
    pub agent_id: String,
    pub agent_generation: i64,
    pub binding_generation: i64,
    pub event_id: String,
    pub room_id: String,
    pub sender_mxid: String,
    pub source_digest: String,
    pub received_at_ms: i64,
    pub event: Value,
}
fn canonical(v: &Value) -> Result<String, DeviceError> {
    Ok(match v {
        Value::Object(o) => {
            let mut pairs = o.iter().collect::<Vec<_>>();
            pairs.sort_by_key(|(k, _)| *k);
            let mut out = Vec::new();
            for (k, v) in pairs {
                out.push(format!(
                    "{}:{}",
                    serde_json::to_string(k).map_err(|_| invalid())?,
                    canonical(v)?
                ));
            }
            format!("{{{}}}", out.join(","))
        }
        Value::Array(a) => format!(
            "[{}]",
            a.iter()
                .map(canonical)
                .collect::<Result<Vec<_>, _>>()?
                .join(",")
        ),
        _ => serde_json::to_string(v).map_err(|_| invalid())?,
    })
}
fn invalid() -> DeviceError {
    error(502, "invalid_ciphertext_response")
}
pub fn source_digest(event: &Value) -> Result<String, DeviceError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(canonical(&json!(["agent-encrypted-inbox-v1", event]))?.as_bytes())
    ))
}
impl Ciphertext {
    pub(super) fn validate(&self, lease: &LeaseRef) -> Result<(), DeviceError> {
        id(&self.id)?;
        id(&self.binding_id)?;
        id(&self.agent_id)?;
        if self.agent_id != lease.agent_id
            || self.agent_generation <= 0
            || self.binding_generation <= 0
            || self.received_at_ms < 0
            || !self.id.starts_with("enc_")
            || !super::super::matrix_identifiers::room_id(&self.room_id)
            || !self.event_id.starts_with('$')
            || self.event_id.len() > 512
            || self
                .event_id
                .chars()
                .any(|c| c.is_control() || c.is_whitespace())
            || !self.sender_mxid.starts_with('@')
            || !self.sender_mxid.contains(':')
            || self.sender_mxid.len() > 512
            || self
                .sender_mxid
                .chars()
                .any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(invalid());
        }
        if self.event.as_object().is_none_or(|o| {
            o.len() != 6
                || o.keys().any(|k| {
                    !matches!(
                        k.as_str(),
                        "event_id" | "room_id" | "sender" | "origin_server_ts" | "type" | "content"
                    )
                })
        }) || self.event["type"] != "m.room.encrypted"
            || self.event["event_id"] != self.event_id
            || self.event["room_id"] != self.room_id
            || self.event["sender"] != self.sender_mxid
            || self.event["origin_server_ts"].as_u64().is_none()
        {
            return Err(invalid());
        }
        let c = &self.event["content"];
        if c.get("m.relates_to").is_some_and(|v| !v.is_object())
            || c["algorithm"] != "m.megolm.v1.aes-sha2"
            || c.as_object().is_none_or(|o| {
                o.keys().any(|k| {
                    !matches!(
                        k.as_str(),
                        "algorithm"
                            | "ciphertext"
                            | "session_id"
                            | "sender_key"
                            | "device_id"
                            | "m.relates_to"
                    )
                })
            })
            || ["ciphertext", "session_id"].iter().any(|k| {
                c[k].as_str()
                    .is_none_or(|s| s.is_empty() || s.chars().any(char::is_control))
            })
            || canonical(&self.event)?.len() > 131072
            || source_digest(&self.event)? != self.source_digest
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ciphertext_wire_requires_original_canonical_commitment_and_exact_headers() {
        let value: Value =
            serde_json::from_str(include_str!("ciphertext-wire-fixture.json")).unwrap();
        let event: Ciphertext = serde_json::from_value(value.clone()).unwrap();
        let lease = LeaseRef {
            agent_id: "agt_fixture".into(),
            epoch: 1,
        };
        event.validate(&lease).unwrap();
        assert_eq!(source_digest(&event.event).unwrap(), value["sourceDigest"]);
        let mut forged = value.clone();
        forged["verified"] = json!(true);
        assert!(serde_json::from_value::<Ciphertext>(forged).is_err());
        for (field, v) in [
            ("sourceDigest", json!("0".repeat(64))),
            ("eventId", json!("$other")),
            ("roomId", json!("!other:test")),
            ("senderMxid", json!("@other:test")),
            ("agentId", json!("agt_other")),
            ("agentGeneration", json!(0)),
            ("bindingGeneration", json!(0)),
        ] {
            let mut forged = value.clone();
            forged[field] = v;
            assert!(
                serde_json::from_value::<Ciphertext>(forged)
                    .unwrap()
                    .validate(&lease)
                    .is_err(),
                "accepted {field}"
            );
        }
        let mut threaded = event.clone();
        threaded.event["content"]["m.relates_to"] =
            json!({"rel_type":"m.thread","event_id":"$root","is_falling_back":false});
        threaded.source_digest = source_digest(&threaded.event).unwrap();
        threaded.validate(&lease).unwrap();
        let device = crate::console::server_login::fixture_device(
            "http://127.0.0.1:9/",
            "owner",
            "@owner:test",
        )
        .0;
        let op = DeviceOperation::ReadCiphertext {
            lease: lease.clone(),
            inbox_id: "enc_other".into(),
        };
        assert!(parse_response(&op, json!({"ciphertext":event}), &device).is_err());
        let op = DeviceOperation::PollCiphertexts {
            lease: lease.clone(),
            binding_id: "bnd_other".into(),
            limit: 4,
        };
        assert!(parse_response(&op, json!({"ciphertexts":[event]}), &device).is_err());
        let mut forged = event.clone();
        forged.event["content"]["body"] = json!("forged plaintext");
        forged.source_digest = source_digest(&forged.event).unwrap();
        assert!(forged.validate(&lease).is_err());
    }
    #[test]
    fn ciphertext_wire_data_cannot_choose_history_urls_filters_or_unbounded_batches() {
        let lease = LeaseRef {
            agent_id: "agt_fixture".into(),
            epoch: 1,
        };
        let poll = DeviceOperation::PollCiphertexts {
            lease: lease.clone(),
            binding_id: "bnd_fixture".into(),
            limit: 4,
        };
        let (path, body) = poll.request().unwrap();
        assert_eq!(path, "ciphertexts/poll");
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"lease":lease,"bindingId":"bnd_fixture","limit":4})
        );
        for limit in [0, 5] {
            assert!(
                DeviceOperation::PollCiphertexts {
                    lease: lease.clone(),
                    binding_id: "bnd_fixture".into(),
                    limit
                }
                .request()
                .is_err()
            );
        }
        assert!(
            DeviceOperation::ReadCiphertext {
                lease,
                inbox_id: "https://untrusted/event".into()
            }
            .request()
            .is_err()
        );
    }
}
