//! Remote device request inspection and irreversible encrypted verdict sends.
use super::*;
use ::hagency::native_owner::remote_approval::{
    REQUEST_TYPE,
    review::{ObservedRequest, RequestPage, VerdictSendPermit},
};
fn now() -> Result<i64, NativeError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| refused())?
        .as_secs()
        .try_into()
        .map_err(|_| refused())
}
pub async fn inspect(
    client: &Client,
    origin: &str,
    owner: &str,
    room_id: &str,
    event_id: &str,
    live: impl Fn() -> Result<(), NativeError>,
) -> Result<Option<ObservedRequest>, NativeError> {
    tokio::time::timeout(Duration::from_secs(30),async {
        let _slot=READS.acquire().await.map_err(|_|refused())?;
        let proof_at=now()?;
        let reviewer=client.device_id().ok_or_else(refused)?.to_string();
        let check=|| {
            live()?;
            if client.homeserver().as_str()!=origin || client.user_id().is_none_or(|u|u.as_str()!=owner) || client.device_id().is_none_or(|d|d.as_str()!=reviewer) {return Err(refused());} Ok(())
        };
        check()?;let id=RoomId::parse(room_id).map_err(|_|refused())?;
        EventId::parse(event_id).map_err(|_|refused())?;
        let user=UserId::parse(owner).map_err(|_|refused())?;
        let room=client.get_room(&id).filter(|r|r.state()==RoomState::Joined).ok_or_else(refused)?;
        private_state(&bounded_get(client,origin,&["_matrix","client","v3","rooms",room_id,"state"]).await?,owner)?;check()?;
        client.encryption().request_user_identity(&user).await.map_err(|_|refused())?;check()?;
        let encrypted=bounded_get(client,origin,&["_matrix","client","v3","rooms",room_id,"event",event_id]).await?;check()?;
        if encrypted["type"]!="m.room.encrypted" || encrypted["event_id"]!=event_id || encrypted["sender"]!=owner {return Err(refused());}
        let raw:Raw<OriginalSyncRoomEncryptedEvent>=Raw::from_json(serde_json::value::to_raw_value(&encrypted).map_err(|_|refused())?);
        let event=room.decrypt_event(&raw,None).await.map_err(|_|refused())?;check()?;
        let info=event.encryption_info().filter(|i|matches!(i.verification_state,VerificationState::Verified) && i.forwarder.is_none() && i.sender.as_str()==owner).ok_or_else(refused)?;
        let sender=info.sender_device.as_ref().ok_or_else(refused)?;
        let content:Value=serde_json::from_str(event.raw().json().get()).map_err(|_|refused())?;
        if content["event_id"]!=event_id || content["sender"]!=owner || content["type"]!="m.room.message" || content["unsigned"].get("redacted_because").is_some() || content.get("room_id").is_some_and(|r|r!=room_id) {return Err(refused());}
        if content["content"]["msgtype"]!=REQUEST_TYPE {return Ok(None);}
        let request:Request=serde_json::from_value(content["content"]["im.hagency.owner.approval"].clone()).map_err(|_|refused())?;
        request.validate().map_err(|_|refused())?;
        if request.owner!=owner || request.approval_room!=room_id || request.publisher_matrix_device!=sender.as_str() {return Err(refused());}
        if sender.as_str()==reviewer {return Ok(None);}
        private_state(&bounded_get(client,origin,&["_matrix","client","v3","rooms",room_id,"state"]).await?,owner)?;check()?;
        let identity=client.encryption().request_user_identity(&user).await.map_err(|_|refused())?.ok_or_else(refused)?;check()?;
        let device=client.encryption().get_device(&user,sender).await.map_err(|_|refused())?.ok_or_else(refused)?;
        let own=client.encryption().get_own_device().await.map_err(|_|refused())?.ok_or_else(refused)?;check()?;
        if !identity.is_verified() || device.is_deleted() || !device.is_verified_with_cross_signing() || own.is_deleted() || !own.is_verified_with_cross_signing() || room.state()!=RoomState::Joined {return Err(refused());}
        let key=device.ed25519_key().ok_or_else(refused)?.to_base64();
        if !matches!(&info.algorithm_info,AlgorithmInfo::MegolmV1AesSha2 {sender_claimed_keys,..} if sender_claimed_keys.get(&DeviceKeyAlgorithm::Ed25519)==Some(&key)) {return Err(refused());}
        if request.proposal.expires<=now()? {return Err(source_error("remote_approval_expired"));}
        Ok(Some(ObservedRequest {observed_at:proof_at,event_id:event_id.into(),sender_ed25519:key,reviewer_matrix_device:reviewer,request}))
    }).await.map_err(|_|refused())?
}
pub async fn list(
    client: &Client,
    origin: &str,
    owner: &str,
    room: &str,
    cursor: Option<&str>,
    live: impl Fn() -> Result<(), NativeError>,
) -> Result<RequestPage, NativeError> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let _slot = SEARCHES.acquire().await.map_err(|_| refused())?;
        live()?;
        RoomId::parse(room).map_err(|_| refused())?;
        if cursor.is_some_and(|c| c.is_empty() || c.len() > 1024 || c.chars().any(char::is_control))
        {
            return Err(refused());
        }
        let mut query = vec![
            ("dir", "b"),
            ("limit", "10"),
            ("filter", r#"{"types":["m.room.encrypted"]}"#),
        ];
        if let Some(cursor) = cursor {
            query.push(("from", cursor));
        }
        // Authenticate account/private scope before requesting any history.
        prepare(client, origin, owner, room, &live).await?;
        let response = bounded_get_query(
            client,
            origin,
            &["_matrix", "client", "v3", "rooms", room, "messages"],
            &query,
        )
        .await?;
        live()?;
        let chunk = response["chunk"]
            .as_array()
            .filter(|c| c.len() <= 10)
            .ok_or_else(refused)?;
        let mut requests = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for event in chunk {
            let id = event["event_id"].as_str().ok_or_else(refused)?;
            if event["type"] != "m.room.encrypted" || event["sender"] != owner || !seen.insert(id) {
                return Err(refused());
            }
            match inspect(client, origin, owner, room, id, &live).await {
                Ok(Some(observed)) => requests.push(observed),
                Ok(None) => {}
                Err(e) if e.code == "remote_approval_expired" => {}
                Err(e) => return Err(e),
            }
        }
        let next_batch = match response.get("end") {
            None | Some(Value::Null) => None,
            Some(Value::String(s))
                if !s.is_empty() && s.len() <= 1024 && Some(s.as_str()) != cursor =>
            {
                Some(s.clone())
            }
            _ => return Err(refused()),
        };
        Ok(RequestPage {
            requests,
            next_batch,
        })
    })
    .await
    .map_err(|_| refused())?
}
pub async fn publish(
    client: &Client,
    origin: &str,
    permit: VerdictSendPermit,
    live: impl Fn() -> Result<(), NativeError>,
) -> Result<String, NativeError> {
    tokio::time::timeout(Duration::from_secs(60), async {
        let _slot = PUBLISHES.acquire().await.map_err(|_| refused())?;
        let r = permit.record();
        let request = &r.request;
        let check = || {
            live()?;
            if now()? >= request.proposal.expires
                || client
                    .device_id()
                    .is_none_or(|d| d.as_str() != r.reviewer_matrix_device)
                || client.user_id().is_none_or(|u| u.as_str() != request.owner)
                || client.homeserver().as_str() != origin
            {
                return Err(refused());
            }
            Ok(())
        };
        check()?;
        let observed = inspect(
            client,
            origin,
            &request.owner,
            &request.approval_room,
            &r.request_event_id,
            &check,
        )
        .await?
        .ok_or_else(refused)?;
        if observed.request != *request
            || observed.sender_ed25519 != r.sender_ed25519
            || observed.request.digest().map_err(|_| refused())? != r.request_digest
        {
            return Err(refused());
        }
        prepare(
            client,
            origin,
            &request.owner,
            &request.approval_room,
            &check,
        )
        .await?;
        let room = client
            .get_room(&RoomId::parse(&request.approval_room).map_err(|_| refused())?)
            .ok_or_else(refused)?;
        let sealed = room
            .seal_for_own_devices("m.room.message", permit.content().map_err(|_| refused())?)
            .await
            .map_err(|_| refused())?;
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
        let txn: matrix_sdk::ruma::OwnedTransactionId = permit.transaction_id().into();
        let sent = sealed
            .send(&txn, || check().is_ok())
            .await
            .map_err(|_| source_error("remote_approval_delivery_unknown"))?;
        Ok(sent.response.event_id.to_string())
    })
    .await
    .map_err(|_| source_error("remote_approval_delivery_unknown"))?
}
