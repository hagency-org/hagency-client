//! Owner-scoped key receipt and crypto evidence. No execution/room/media grant.
use super::*;
use std::sync::{Arc, Mutex};
use matrix_sdk::{
    HttpRequestFence,
    config::SyncSettings,
    deserialized_responses::{AlgorithmInfo, VerificationState},
    ruma::{
        RoomId, events::room::encrypted::OriginalSyncRoomEncryptedEvent, serde::Raw,
        DeviceKeyAlgorithm,
    },
};
#[derive(Clone)]
struct OwnerPin {
    user: String,
    device: String,
    master: String,
}
pub(super) struct OwnerContext {
    client: Client,
    live: Arc<dyn Fn() -> bool + Send + Sync>,
    state: Mutex<(Option<OwnerPin>, Option<String>)>,
}
impl OwnerContext {
    pub(super) fn status(&self) -> Value {
        self.state.lock().map_or_else(|_|json!({"phase":"unavailable","error":"agent_crypto_receive_unavailable"}),|s|json!({"phase":if s.0.is_some()&&s.1.is_none(){"ready"}else{"unavailable"},"error":s.1}))
    }
}
fn refused() -> NativeError {
    error("agent_crypto_receive_unavailable")
}
async fn owner_pin(
    authority: &AgentCryptoAuthority,
    context: &OwnerContext,
) -> Result<OwnerPin, NativeError> {
    let d = authority.device().await?;
    let user = UserId::parse(d.owner_mxid()).map_err(|_| refused())?;
    let id = context.client.device_id().ok_or_else(refused)?.to_owned();
    if context.client.user_id() != Some(&user)
        || context.client.homeserver().as_str() != d.origin()
        || !(context.live)()
    {
        return Err(refused());
    }
    let client = context.client.clone();
    let origin = d.origin().to_owned();
    let owner = user.clone();
    let device = id.clone();
    let auth = authority.clone();
    let live = context.live.clone();
    let fence = Arc::new(HttpRequestFence::new(move || {
        !auth.is_retired()
            && d.bearer().is_ok()
            && live()
            && client.user_id() == Some(&owner)
            && client.device_id() == Some(device.as_ref())
            && client.homeserver().as_str() == origin
    }));
    let _retire = Retirement(fence.clone());
    let identity = context
        .client
        .encryption()
        .request_user_identity_with_fence(&user, &id, fence.clone())
        .await
        .map_err(|_| error("agent_crypto_owner_query_unavailable"))?
        .ok_or_else(|| error("agent_crypto_owner_keys_unverified"))?;
    let device = context
        .client
        .encryption()
        .get_own_device()
        .await
        .map_err(|_| refused())?
        .ok_or_else(refused)?;
    if !identity.is_verified() || device.is_deleted() || !device.is_verified_with_cross_signing() {
        return Err(error("agent_crypto_owner_keys_unverified"));
    }
    let master = identity
        .master_key()
        .get_first_key()
        .ok_or_else(refused)?
        .to_base64();
    authority.device().await?;
    fence.retire_and_drain().await.map_err(|_| refused())?;
    Ok(OwnerPin {
        user: user.to_string(),
        device: id.to_string(),
        master,
    })
}
impl AgentCryptoSession {
    async fn owner_peer(
        &self,
        client: &Client,
        pin: &OwnerPin,
        context: &OwnerContext,
        sign: bool,
    ) -> Result<(), NativeError> {
        let d = self.current(client).await?;
        let authority = self.authority.clone();
        let live = context.live.clone();
        let owner_sdk = context.client.clone();
        let owner = pin.user.clone();
        let matrix_device = pin.device.clone();
        let origin = d.origin().to_owned();
        let fence = Arc::new(HttpRequestFence::new(move || {
            !authority.is_retired()
                && d.bearer().is_ok()
                && live()
                && owner_sdk.user_id().is_some_and(|u| u.as_str() == owner)
                && owner_sdk
                    .device_id()
                    .is_some_and(|id| id.as_str() == matrix_device)
                && owner_sdk.homeserver().as_str() == origin
        }));
        let _retire = Retirement(fence.clone());
        let user = UserId::parse(&pin.user).map_err(|_| refused())?;
        let device: &matrix_sdk::ruma::DeviceId = pin.device.as_str().into();
        let peer = client
            .encryption()
            .request_user_identity_with_fence(&user, device, fence.clone())
            .await
            .map_err(|_| error("agent_crypto_owner_query_unavailable"))?
            .ok_or_else(refused)?;
        if peer
            .master_key()
            .get_first_key()
            .map(|k| k.to_base64())
            .as_deref()
            != Some(&pin.master)
        {
            return Err(error("agent_crypto_owner_identity_changed"));
        }
        if sign && !peer.is_verified() {
            peer.verify_with_request_fence(fence.clone())
                .await
                .map_err(|_| error("agent_crypto_owner_signature_unavailable"))?;
        }
        let peer = client
            .encryption()
            .request_user_identity_with_fence(&user, device, fence.clone())
            .await
            .map_err(|_| error("agent_crypto_owner_query_unavailable"))?
            .ok_or_else(refused)?;
        if !peer.is_verified()
            || peer
                .master_key()
                .get_first_key()
                .map(|k| k.to_base64())
                .as_deref()
                != Some(&pin.master)
        {
            return Err(error("agent_crypto_owner_identity_changed"));
        }
        self.current(client).await?;
        fence.retire_and_drain().await.map_err(|_| refused())?;
        Ok(())
    }
    async fn prepare_owner_receive(&self, context: &OwnerContext) -> Result<(), NativeError> {
        if !self.owner_receive {
            return Err(refused());
        }
        let pin = owner_pin(&self.authority, context).await?;
        let resources = self.resources.lock().await;
        let client = &resources.as_ref().ok_or_else(refused)?.client;
        self.owner_peer(client, &pin, context, true).await?;
        self.current(client).await?;
        client
            .sync_once(SyncSettings::default().timeout(Duration::from_millis(1)))
            .await
            .map_err(|_| error("agent_crypto_key_sync_unavailable"))?;
        self.current(client).await?;
        *context.state.lock().map_err(|_| refused())? = (Some(pin), None);
        Ok(())
    }
    pub(super) async fn decrypt_owner_event(
        &self,
        context: &OwnerContext,
        room: &str,
        event: &Value,
    ) -> Result<Value, NativeError> {
        let result = async {
            let initialize = { context.state.lock().map_err(|_| refused())?.0.is_none() };
            if initialize {
                self.prepare_owner_receive(context).await?;
            }
            let pin = context
                .state
                .lock()
                .map_err(|_| refused())?
                .0
                .clone()
                .ok_or_else(refused)?;
            let actual = owner_pin(&self.authority, context).await?;
            if actual.master != pin.master || actual.user != pin.user || actual.device != pin.device
            {
                return Err(error("agent_crypto_owner_identity_changed"));
            }
            let room = RoomId::parse(room).map_err(|_| error("agent_crypto_invalid_event"))?;
            if event["type"] != "m.room.encrypted"
                || event["room_id"] != room.as_str()
                || event["sender"] != pin.user
            {
                return Err(error("agent_crypto_invalid_event"));
            }
            let resources = self.resources.lock().await;
            let client = &resources.as_ref().ok_or_else(refused)?.client;
            self.owner_peer(client, &pin, context, false).await?;
            self.current(client).await?;
            client
                .sync_once(SyncSettings::default().timeout(Duration::from_millis(1)))
                .await
                .map_err(|_| refused())?;
            self.current(client).await?;
            let raw: Raw<OriginalSyncRoomEncryptedEvent> =
                Raw::from_json(serde_json::value::to_raw_value(event).map_err(|_| refused())?);
            let plain = client
                .encryption()
                .decrypt_received_room_event(&room, &raw)
                .await
                .map_err(|_| error("agent_crypto_event_not_decryptable"))?;
            let info = plain
                .encryption_info()
                .filter(|i| {
                    matches!(i.verification_state, VerificationState::Verified)
                        && i.forwarder.is_none()
                        && i.sender.as_str() == pin.user
                })
                .ok_or_else(|| error("agent_crypto_unverified_sender"))?;
            let sender = info
                .sender_device
                .as_ref()
                .ok_or_else(|| error("agent_crypto_unverified_sender"))?;
            let peer = client
                .encryption()
                .request_user_identity_with_fence(&info.sender, sender, self.retirement.0.clone())
                .await
                .map_err(|_| refused())?
                .ok_or_else(refused)?;
            if !peer.is_verified()
                || peer
                    .master_key()
                    .get_first_key()
                    .map(|k| k.to_base64())
                    .as_deref()
                    != Some(&pin.master)
            {
                return Err(error("agent_crypto_unverified_sender"));
            }
            let device = client
                .encryption()
                .get_device(&info.sender, sender)
                .await
                .map_err(|_| refused())?
                .ok_or_else(refused)?;
            if device.is_deleted() || !device.is_verified_with_cross_signing() {
                return Err(error("agent_crypto_unverified_sender"));
            }
            match &info.algorithm_info {
                AlgorithmInfo::MegolmV1AesSha2 {
                    curve25519_key,
                    sender_claimed_keys,
                    ..
                } if device.curve25519_key().map(|k| k.to_base64()).as_deref()
                    == Some(curve25519_key.as_str())
                    && device.ed25519_key().map(|k| k.to_base64()).as_ref()
                        == sender_claimed_keys.get(&DeviceKeyAlgorithm::Ed25519) => {}
                _ => return Err(error("agent_crypto_unverified_sender")),
            }
            self.current(client).await?;
            if !(context.live)() {
                return Err(refused());
            }
            let value: Value =
                serde_json::from_str(plain.raw().json().get()).map_err(|_| refused())?;
            if event["content"].get("m.relates_to").is_some()
                && value["content"].get("m.relates_to").is_none()
            {
                return Err(error("agent_crypto_unauthenticated_relation"));
            }
            if value["sender"] != pin.user
                || value["event_id"] != event["event_id"]
                || value["type"] != "m.room.message"
                || value["unsigned"].get("redacted_because").is_some()
            {
                return Err(error("agent_crypto_invalid_event"));
            }
            Ok(value)
        }
        .await;
        if let Ok(mut state) = context.state.lock() {
            state.1 = result.as_ref().err().map(|e| e.code.clone());
        }
        result
    }
}
pub async fn prepare_managed_receiver(
    context: AgentCryptoContext,
    owner: Client,
    live: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<Arc<dyn AgentCryptoTransport>, NativeError> {
    let session = AgentCryptoSession::open_authority(
        context.authority(),
        context.agent(),
        context.generation(),
        context.root(),
        context.passphrase(),
        true,
    )
    .await?;
    let identity = match session.bootstrap().await {
        Ok(i) => i,
        Err(e) => {
            session.close().await?;
            return Err(e);
        }
    };
    let receiver = OwnerContext {
        client: owner,
        live,
        state: Mutex::new((None, None)),
    };
    if let Err(e) = session.prepare_owner_receive(&receiver).await {
        *receiver.state.lock().map_err(|_| refused())? = (None, Some(e.code));
    }
    Ok(Arc::new(Managed {
        session,
        identity,
        receiver: Some(receiver),
    }))
}
