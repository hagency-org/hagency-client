//! Read-only SDK evidence for native owner approvals. Wire JSON is never a
//! device-verification proof. No publication, runtime start or grant happens here.
use super::{NativeError, source_error};
use ::hagency::native_owner::remote_approval::{
    ObservedVerdict, Publication, Request, VERDICT_TYPE, Verdict,
};
use matrix_sdk::{
    Client, RoomState,
    deserialized_responses::{AlgorithmInfo, TimelineEvent, VerificationState},
    ruma::{
        DeviceKeyAlgorithm, EventId, RoomId, UserId, serde::Raw,
        events::room::encrypted::OriginalSyncRoomEncryptedEvent,
    },
};
use serde_json::Value;
use std::time::Duration;
static READS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
fn refused() -> NativeError {
    source_error("remote_approval_unverified")
}
fn private_state(state: &Value, owner: &str) -> Result<(), NativeError> {
    let events = state.as_array().ok_or_else(refused)?;
    let mut keys = std::collections::BTreeSet::new();
    for event in events {
        let typ = event["type"].as_str().ok_or_else(refused)?;
        let key = event["state_key"].as_str().ok_or_else(refused)?;
        if !keys.insert((typ, key)) {
            return Err(refused());
        }
    }
    let content = |typ: &str| {
        events
            .iter()
            .find(|e| e["type"] == typ && e["state_key"] == "")
            .map(|e| &e["content"])
    };
    if content("m.room.create").is_none_or(|c| c["type"] == "m.space")
        || content("m.room.encryption").is_none_or(|c| c["algorithm"] != "m.megolm.v1.aes-sha2")
        || content("m.room.join_rules").is_none_or(|c| c["join_rule"] != "invite")
        || content("m.room.guest_access").is_some_and(|c| c["guest_access"] != "forbidden")
        || content("m.room.history_visibility")
            .is_none_or(|c| !matches!(c["history_visibility"].as_str(), Some("joined" | "invited")))
    {
        return Err(refused());
    }
    let mut joined = false;
    for member in events.iter().filter(|e| e["type"] == "m.room.member") {
        let user = member["state_key"].as_str().ok_or_else(refused)?;
        let membership = member["content"]["membership"]
            .as_str()
            .ok_or_else(refused)?;
        if user == owner {
            joined = membership == "join";
        } else if !matches!(membership, "leave" | "ban") {
            return Err(refused());
        }
    }
    if !joined {
        return Err(refused());
    }
    Ok(())
}
async fn bounded_get(
    client: &Client,
    origin: &str,
    segments: &[&str],
) -> Result<Value, NativeError> {
    let mut url = url::Url::parse(origin).map_err(|_| refused())?;
    url.path_segments_mut()
        .map_err(|_| refused())?
        .clear()
        .extend(segments);
    let mut response = client
        .http_client()
        .get(url.clone())
        .timeout(Duration::from_secs(8))
        .bearer_auth(client.access_token().ok_or_else(refused)?)
        .send()
        .await
        .map_err(|_| refused())?
        .error_for_status()
        .map_err(|_| refused())?;
    if response.url() != &url || response.content_length().is_some_and(|n| n > 128 * 1024) {
        return Err(refused());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| refused())? {
        if bytes.len() + chunk.len() > 128 * 1024 {
            return Err(refused());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| refused())
}
fn envelope(
    event: &TimelineEvent,
    request: &Request,
    digest: &str,
    request_event: &str,
    verdict_event: &str,
    device_key: &str,
    observed_at: i64,
) -> Result<ObservedVerdict, NativeError> {
    let encryption = event.encryption_info().ok_or_else(refused)?;
    if !matches!(encryption.verification_state, VerificationState::Verified)
        || encryption.forwarder.is_some()
        || encryption.sender.as_str() != request.owner
    {
        return Err(refused());
    }
    let device = encryption.sender_device.as_ref().ok_or_else(refused)?;
    if device.as_str() == request.publisher_matrix_device {
        return Err(refused());
    }
    match &encryption.algorithm_info {
        AlgorithmInfo::MegolmV1AesSha2 {
            sender_claimed_keys,
            ..
        } if sender_claimed_keys
            .get(&DeviceKeyAlgorithm::Ed25519)
            .is_some_and(|key| key == device_key) => {}
        _ => return Err(refused()),
    }
    let raw: Value = serde_json::from_str(event.raw().json().get()).map_err(|_| refused())?;
    if raw["event_id"] != verdict_event
        || raw["sender"] != request.owner
        || raw["type"] != "m.room.message"
        || raw
            .get("room_id")
            .is_some_and(|r| r != &request.approval_room)
        || raw["unsigned"].get("redacted_because").is_some()
        || raw["content"]["msgtype"] != VERDICT_TYPE
        || raw["content"]["m.relates_to"]["rel_type"] == "m.replace"
        || raw["content"]["m.relates_to"]["m.in_reply_to"]["event_id"] != request_event
    {
        return Err(refused());
    }
    let verdict: Verdict =
        serde_json::from_value(raw["content"]["im.hagency.owner.approval"].clone())
            .map_err(|_| refused())?;
    if verdict.approval_id != request.approval_id || verdict.request_digest != digest {
        return Err(refused());
    }
    Ok(ObservedVerdict {
        observed_at,
        sender: request.owner.clone(),
        sender_device: device.to_string(),
        sender_ed25519: device_key.into(),
        room: request.approval_room.clone(),
        event_id: verdict_event.into(),
        request_event_id: request_event.into(),
        verdict,
    })
}
pub(super) async fn observe(
    client: &Client,
    origin: &str,
    publication: &Publication,
    request_event: &str,
    verdict_event: &str,
    live: impl Fn() -> Result<(), NativeError>,
) -> Result<ObservedVerdict, NativeError> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let _permit = READS.acquire().await.map_err(|_| refused())?;
        let proof_at: i64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| refused())?
            .as_secs()
            .try_into()
            .map_err(|_| refused())?;
        let request = publication.request();
        request.validate().map_err(|_| refused())?;
        let check = || {
            live()?;
            if client.homeserver().as_str() != origin
                || client.user_id().is_none_or(|u| u.as_str() != request.owner)
                || client
                    .device_id()
                    .is_none_or(|d| d.as_str() != request.publisher_matrix_device)
            {
                return Err(source_error("matrix_account_changed"));
            }
            Ok(())
        };
        check()?;
        let room_id = RoomId::parse(&request.approval_room).map_err(|_| refused())?;
        let owner = UserId::parse(&request.owner).map_err(|_| refused())?;
        let _ = EventId::parse(request_event).map_err(|_| refused())?;
        let _ = EventId::parse(verdict_event).map_err(|_| refused())?;
        let room = client
            .get_room(&room_id)
            .filter(|r| r.state() == RoomState::Joined)
            .ok_or_else(refused)?;
        private_state(
            &bounded_get(
                client,
                origin,
                &[
                    "_matrix",
                    "client",
                    "v3",
                    "rooms",
                    &request.approval_room,
                    "state",
                ],
            )
            .await?,
            &request.owner,
        )?;
        check()?;
        // Refresh before decrypting, then refresh again before returning evidence.
        client
            .encryption()
            .request_user_identity(&owner)
            .await
            .map_err(|_| refused())?;
        check()?;
        let encrypted = bounded_get(
            client,
            origin,
            &[
                "_matrix",
                "client",
                "v3",
                "rooms",
                &request.approval_room,
                "event",
                verdict_event,
            ],
        )
        .await?;
        check()?;
        if encrypted["type"] != "m.room.encrypted"
            || encrypted["event_id"] != verdict_event
            || encrypted["sender"] != request.owner
        {
            return Err(refused());
        }
        let raw: Raw<OriginalSyncRoomEncryptedEvent> =
            Raw::from_json(serde_json::value::to_raw_value(&encrypted).map_err(|_| refused())?);
        let event = room
            .decrypt_event(&raw, None)
            .await
            .map_err(|_| refused())?;
        check()?;
        let device = event
            .encryption_info()
            .filter(|i| matches!(i.verification_state, VerificationState::Verified))
            .and_then(|i| i.sender_device.clone())
            .ok_or_else(refused)?;
        private_state(
            &bounded_get(
                client,
                origin,
                &[
                    "_matrix",
                    "client",
                    "v3",
                    "rooms",
                    &request.approval_room,
                    "state",
                ],
            )
            .await?,
            &request.owner,
        )?;
        check()?;
        let identity = client
            .encryption()
            .request_user_identity(&owner)
            .await
            .map_err(|_| refused())?
            .ok_or_else(refused)?;
        check()?;
        let current = client
            .encryption()
            .get_device(&owner, &device)
            .await
            .map_err(|_| refused())?
            .ok_or_else(refused)?;
        check()?;
        if room.state() != RoomState::Joined {
            return Err(refused());
        }
        if !identity.is_verified()
            || !current.is_verified()
            || !current.is_verified_with_cross_signing()
            || current.is_deleted()
        {
            return Err(refused());
        }
        let key = current.ed25519_key().ok_or_else(refused)?.to_base64();
        envelope(
            &event,
            request,
            publication.request_digest(),
            request_event,
            verdict_event,
            &key,
            proof_at,
        )
    })
    .await
    .map_err(|_| refused())?
}
#[cfg(test)]
mod tests;
