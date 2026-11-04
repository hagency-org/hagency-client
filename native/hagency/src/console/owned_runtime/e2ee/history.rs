use super::*;
use matrix_sdk::{deserialized_responses::TimelineEvent, room::MessagesOptions};
use serde_json::{Value, json};

pub(in crate::console) struct HistoryBatch {
    pub events: Vec<Value>,
    pub next: Option<String>,
}

impl AgentDevice {
    /// The SDK enforces this Agent's membership/history visibility and decrypts
    /// with its own keys. The caller pins the room and anchor from admitted work.
    pub(in crate::console) async fn history_start(
        &self,
        room: &str,
        anchor: &str,
    ) -> Result<Option<String>, RuntimeError> {
        let room_id = OwnedRoomId::try_from(room).map_err(|_| RuntimeError::Request)?;
        let event_id = OwnedEventId::try_from(anchor).map_err(|_| RuntimeError::Request)?;
        let room = self
            .client
            .get_room(&room_id)
            .ok_or(RuntimeError::Transport)?;
        let context = room
            .event_with_context(&event_id, false, 0u32.into(), None)
            .await
            .map_err(|_| RuntimeError::Transport)?;
        if context
            .event
            .as_ref()
            .and_then(|e| e.kind.parse_event_id())
            .as_ref()
            != Some(&event_id)
        {
            return Err(RuntimeError::Transport);
        }
        Ok(context.prev_batch_token)
    }

    pub(in crate::console) async fn history_page(
        &self,
        room: &str,
        from: &str,
    ) -> Result<HistoryBatch, RuntimeError> {
        let room_id = OwnedRoomId::try_from(room).map_err(|_| RuntimeError::Request)?;
        let room = self
            .client
            .get_room(&room_id)
            .ok_or(RuntimeError::Transport)?;
        let mut options = MessagesOptions::backward().from(from);
        options.limit = 50u32.into();
        let page = room
            .messages(options)
            .await
            .map_err(|_| RuntimeError::Transport)?;
        if page.end.as_deref() == Some(from) {
            return Err(RuntimeError::Transport);
        }
        Ok(HistoryBatch {
            events: page.chunk.iter().filter_map(history_event).collect(),
            next: page.end,
        })
    }
}

fn history_event(event: &TimelineEvent) -> Option<Value> {
    let raw: Value = serde_json::from_str(event.kind.raw().json().get()).ok()?;
    let id = raw["event_id"].as_str()?;
    let sender = raw["sender"].as_str()?;
    let mut record = json!({"eventId":id,"sender":sender,"timestamp":raw["origin_server_ts"]});
    let unavailable = match &event.kind {
        TimelineEventKind::UnableToDecrypt { .. } => true,
        TimelineEventKind::Decrypted(d) => {
            d.encryption_info.sender.as_str() != sender
                || matches!(
                    d.encryption_info.verification_state,
                    VerificationState::Unverified(
                        VerificationLevel::None(_)
                            | VerificationLevel::MismatchedSender
                            | VerificationLevel::VerificationViolation
                    )
                )
        }
        TimelineEventKind::PlainText { .. } => false,
    };
    if unavailable {
        record["status"] = "unavailable_encrypted_message".into();
        return Some(record);
    }
    if raw["unsigned"].get("redacted_because").is_some() {
        record["status"] = "redacted".into();
        return Some(record);
    }
    if raw["type"] == "m.room.redaction" {
        record["status"] = "redaction".into();
        record["target"] = raw
            .get("redacts")
            .unwrap_or(&raw["content"]["redacts"])
            .clone();
        return Some(record);
    }
    if raw["type"] != "m.room.message" {
        return None;
    }
    let content = &raw["content"];
    record["body"] = content["body"].clone();
    record["messageType"] = content["msgtype"].clone();
    if content["m.relates_to"]["rel_type"] == "m.replace" {
        record["status"] = "edit".into();
        record["target"] = content["m.relates_to"]["event_id"].clone();
        record["body"] = content["m.new_content"]["body"].clone();
    }
    if let Some(root) = content["m.relates_to"]["event_id"].as_str() {
        record["relatedEventId"] = root.into();
    }
    Some(record)
}
