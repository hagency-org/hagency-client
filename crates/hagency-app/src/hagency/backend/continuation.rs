use super::{NativeError, source_error};
use ::hagency::native_owner::ContinuationMessage;
pub(super) async fn send(
    client: &matrix_sdk::Client,
    expected_origin: &str,
    expected_owner: &str,
    message: ContinuationMessage,
    live_check: impl Fn() -> Result<(), NativeError>,
) -> Result<String, NativeError> {
    live_check()?;
    if client
        .user_id()
        .is_none_or(|id| id.as_str() != message.owner)
        || message.owner != expected_owner
        || client.homeserver().as_str() != expected_origin
    {
        return Err(source_error("matrix_account_changed"));
    }
    let room_id = matrix_sdk::ruma::RoomId::parse(&message.room)
        .map_err(|_| source_error("invalid_room_id"))?;
    let agent = matrix_sdk::ruma::UserId::parse(&message.agent_mxid)
        .map_err(|_| source_error("invalid_agent_id"))?;
    let room = client
        .get_room(&room_id)
        .ok_or_else(|| source_error("room_join_required"))?;
    if room.state() != matrix_sdk::RoomState::Joined || room.is_space() {
        return Err(source_error("room_join_required"));
    }
    if message.require_encrypted && !room.encryption_state().is_encrypted() {
        return Err(source_error("encrypted_room_required"));
    }
    let mut content = serde_json::json!({"msgtype":"m.text","body":message.body,"m.mentions":{"user_ids":[agent]}});
    if message.thread != message.room {
        let thread = matrix_sdk::ruma::EventId::parse(&message.thread)
            .map_err(|_| source_error("invalid_thread_id"))?;
        content["m.relates_to"] =
            serde_json::json!({"rel_type":"m.thread","event_id":thread,"is_falling_back":false});
    }
    let content = serde_json::value::to_raw_value(&content)
        .map_err(|_| source_error("invalid_continuation_message"))?;
    let transaction_id: matrix_sdk::ruma::OwnedTransactionId = message.transaction_id.into();
    live_check()?;
    let send = room
        .send_raw("m.room.message", content)
        .with_transaction_id(&transaction_id);
    let send = if message.require_encrypted {
        send.require_encryption()
    } else {
        send
    };
    let response = send
        .await
        .map_err(|_| source_error("continuation_delivery_unknown"))?;
    // Retain the actual SDK acceptance even if the account changes just
    // afterwards. NativeOwner rechecks identity before returning it.
    Ok(response.response.event_id.to_string())
}
#[cfg(test)]
mod tests;
