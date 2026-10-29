//! The executor's own Matrix device for an encrypted owner-direct Room. The
//! server routes only ciphertext and records receipts: this device decrypts the
//! owner's request for the local turn and sends the encrypted answer itself.
//! Decrypted text stays in memory, the local ledger and the provider session.
use super::super::device_execution::E2eeDeviceGrant;
use super::*;
use matrix_sdk::{
    Client, SessionMeta, SessionTokens,
    authentication::matrix::MatrixSession,
    config::SyncSettings,
    deserialized_responses::{DecryptedRoomEvent, TimelineEventKind},
    ruma::{
        OwnedEventId, OwnedRoomId, OwnedTransactionId, OwnedUserId,
        api::error::ErrorKind,
        events::{
            AnyMessageLikeEvent, AnyTimelineEvent, MessageLikeEvent,
            reaction::ReactionEventContent,
            relation::{Annotation, Thread},
            room::message::{MessageType, Relation, RoomMessageEventContent},
        },
    },
    store::RoomLoadSettings,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        LazyLock,
        atomic::{AtomicBool, Ordering},
    },
};

/// Room keys travel as to-device messages and can arrive after the routed event.
const KEY_WAIT: Duration = Duration::from_secs(60);
/// Encryption grows content by about a third; stay inside the 64 KiB event.
const MARKDOWN_BYTES: usize = 16 * 1024;
const PLAIN_BYTES: usize = 42 * 1024;

/// One device per Agent per process. A new grant rotates the device's token, so
/// a second client for the same Agent would revoke the first.
static DEVICES: LazyLock<Mutex<HashMap<String, Arc<AgentDevice>>>> =
    LazyLock::new(Default::default);

pub(super) struct AgentDevice {
    client: Client,
    revoked: Arc<AtomicBool>,
    sync: tokio::task::AbortHandle,
}
impl Drop for AgentDevice {
    fn drop(&mut self) {
        self.sync.abort();
    }
}

/// The Agent device must exist before the owner's client encrypts for it, so an
/// owner-direct worker opens it at start. Best effort: plaintext Rooms never
/// need it, and an encrypted request retries the grant itself.
pub(super) async fn prepare<T: Transport>(
    transport: &T,
    lease: &LeaseRef,
    config: &StartConfig,
    cancel: &mut watch::Receiver<bool>,
) {
    match tokio::time::timeout(
        Duration::from_secs(20),
        device(transport, lease, config, cancel),
    )
    .await
    {
        Ok(Ok(_)) => tracing::debug!(stage = "agent_device", "Agent device ready"),
        Ok(Err(error)) => {
            tracing::warn!(stage = "agent_device", %error, "Agent device unavailable")
        }
        Err(_) => tracing::warn!(stage = "agent_device", "Agent device start timed out"),
    }
}

pub(super) async fn device<T: Transport>(
    transport: &T,
    lease: &LeaseRef,
    config: &StartConfig,
    cancel: &mut watch::Receiver<bool>,
) -> Result<Arc<AgentDevice>, RuntimeError> {
    let mut devices = DEVICES.lock().await;
    if let Some(device) = devices.get(&config.agent_id)
        && !device.revoked.load(Ordering::Acquire)
    {
        return Ok(device.clone());
    }
    let Response::E2eeDevice(grant) = operation(
        transport,
        cancel,
        Op::E2eeDevice {
            lease: lease.clone(),
        },
    )
    .await?
    else {
        return Err(RuntimeError::Transport);
    };
    let directory = config.profile.home.parent().ok_or(RuntimeError::Profile)?;
    let device = Arc::new(AgentDevice::open(grant, directory, &config.agent_id).await?);
    devices.insert(config.agent_id.clone(), device.clone());
    Ok(device)
}

fn private_directory(path: &Path) -> Result<(), RuntimeError> {
    hagency_store::private::directory(path).map_err(|_| RuntimeError::Profile)
}
/// The SQLite stores hold the device's Olm account and room keys, encrypted
/// with a random passphrase that never leaves this owner's private directory.
fn store_passphrase(root: &Path) -> Result<String, RuntimeError> {
    let path = root.join("store-key");
    match hagency_store::private::read_secret(&path) {
        Ok(bytes) => String::from_utf8(bytes)
            .ok()
            .filter(|key| key.len() == 64)
            .ok_or(RuntimeError::Profile),
        Err(hagency_store::Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut key = [0u8; 32];
            getrandom::fill(&mut key).map_err(|_| RuntimeError::Profile)?;
            let key = key.iter().map(|b| format!("{b:02x}")).collect::<String>();
            hagency_store::private::write_new(&path, key.as_bytes())
                .map_err(|_| RuntimeError::Profile)?;
            Ok(key)
        }
        Err(_) => Err(RuntimeError::Profile),
    }
}
fn unknown_token(error: &matrix_sdk::Error) -> bool {
    matches!(
        error.client_api_error_kind(),
        Some(ErrorKind::UnknownToken(_))
    )
}
fn txn(kind: &str, dispatch: &str) -> OwnedTransactionId {
    format!("{kind}_{:x}", Sha256::digest(dispatch.as_bytes())).into()
}

impl AgentDevice {
    async fn open(
        grant: E2eeDeviceGrant,
        directory: &Path,
        agent: &str,
    ) -> Result<Self, RuntimeError> {
        let user_id =
            OwnedUserId::try_from(grant.user_id.as_str()).map_err(|_| RuntimeError::Transport)?;
        let devices = directory.join("agent-devices");
        private_directory(&devices)?;
        let root = devices.join(format!("{:x}", Sha256::digest(agent.as_bytes())));
        private_directory(&root)?;
        let passphrase = store_passphrase(&root)?;
        // A new execution device generation is a new Matrix device and account.
        let store = root.join(&grant.device_id);
        private_directory(&store)?;
        let client = Client::builder()
            .homeserver_url(&grant.homeserver)
            .sqlite_store(&store, Some(&passphrase))
            .build()
            .await
            .map_err(|_| RuntimeError::Profile)?;
        let session = MatrixSession {
            meta: SessionMeta {
                user_id,
                device_id: grant.device_id.as_str().into(),
            },
            tokens: SessionTokens {
                access_token: grant.access_token,
                refresh_token: None,
            },
        };
        client
            .matrix_auth()
            .restore_session(session, RoomLoadSettings::default())
            .await
            .map_err(|_| RuntimeError::Profile)?;
        // The first sync uploads this device's identity and one-time keys, so
        // the owner's client starts sharing room keys with it.
        client
            .sync_once(SyncSettings::default().timeout(Duration::ZERO))
            .await
            .map_err(|_| RuntimeError::Transport)?;
        let revoked = Arc::new(AtomicBool::new(false));
        let sync = tokio::spawn(sync(client.clone(), revoked.clone())).abort_handle();
        Ok(Self {
            client,
            revoked,
            sync,
        })
    }

    /// Returns the owner's request text, or None when this device can never
    /// read it: not a text message from the requester, or keys never arrived.
    pub(super) async fn read_request(
        &self,
        dispatch: &inbox::Dispatch,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<Option<String>, RuntimeError> {
        let room_id =
            OwnedRoomId::try_from(dispatch.room_id.as_str()).map_err(|_| RuntimeError::Request)?;
        let event_id = OwnedEventId::try_from(dispatch.event_id.as_str())
            .map_err(|_| RuntimeError::Request)?;
        let deadline = Instant::now() + KEY_WAIT;
        while Instant::now() < deadline {
            if let Some(room) = self.client.get_room(&room_id)
                && let Ok(event) = room.event(&event_id, None).await
            {
                match &event.kind {
                    TimelineEventKind::Decrypted(decrypted) => {
                        return Ok(request_text(decrypted, &dispatch.requester_mxid));
                    }
                    TimelineEventKind::PlainText { event }
                        if event.get_field::<String>("type").ok().flatten().as_deref()
                            != Some("m.room.encrypted") =>
                    {
                        return Ok(None);
                    }
                    _ => {}
                }
            }
            if self.revoked.load(Ordering::Acquire) {
                return Err(RuntimeError::Transport);
            }
            tokio::select! {biased; _=canceled(cancel)=>return Err(RuntimeError::Stopped), _=tokio::time::sleep(Duration::from_secs(2))=>{}}
        }
        Ok(None)
    }

    /// Sends exactly one encrypted event per transaction ID. Retrying after an
    /// interruption reuses it, so the homeserver returns the original event.
    async fn send(
        &self,
        dispatch: &inbox::Dispatch,
        txn: OwnedTransactionId,
        content: impl matrix_sdk::ruma::events::MessageLikeEventContent,
    ) -> Result<String, RuntimeError> {
        let room_id =
            OwnedRoomId::try_from(dispatch.room_id.as_str()).map_err(|_| RuntimeError::Request)?;
        let room = self
            .client
            .get_room(&room_id)
            .ok_or(RuntimeError::Transport)?;
        // Never fall back to plaintext: the server delivers plaintext replies.
        if !room
            .latest_encryption_state()
            .await
            .map_err(|_| RuntimeError::Transport)?
            .is_encrypted()
        {
            return Err(RuntimeError::Request);
        }
        match room.send(content).with_transaction_id(txn).await {
            Ok(sent) => Ok(sent.response.event_id.to_string()),
            Err(error) => {
                if unknown_token(&error) {
                    self.revoked.store(true, Ordering::Release);
                }
                tracing::warn!(stage = "agent_device", %error, "Encrypted send failed");
                Err(RuntimeError::Transport)
            }
        }
    }

    pub(super) async fn send_reply(
        &self,
        dispatch: &inbox::Dispatch,
        body: &str,
    ) -> Result<String, RuntimeError> {
        let content = reply_content(dispatch, body)?;
        self.send(dispatch, txn("hagency", &dispatch.id), content)
            .await
    }

    /// Best effort, like the server's processing reaction for plaintext Rooms.
    pub(super) async fn mark_processing(&self, dispatch: &inbox::Dispatch) {
        let Ok(event) = OwnedEventId::try_from(dispatch.event_id.as_str()) else {
            return;
        };
        let reaction = ReactionEventContent::new(Annotation::new(event, "👀".into()));
        let _ = self
            .send(dispatch, txn("hagency_seen", &dispatch.id), reaction)
            .await;
    }

    /// Tells the owner, inside the encrypted Room, that a request was unreadable.
    pub(super) async fn notice_unreadable(&self, dispatch: &inbox::Dispatch) {
        let mut content = RoomMessageEventContent::notice_plain(
            "🔐 这条加密消息在 Agent 设备上无法解密（可能是在 Agent 设备加入前发送的）。请重新发送一次。",
        );
        if dispatch.thread_root != dispatch.room_id
            && let Ok(root) = OwnedEventId::try_from(dispatch.thread_root.as_str())
        {
            content.relates_to = Some(Relation::Thread(Thread::plain(root.clone(), root)));
        }
        let _ = self
            .send(dispatch, txn("hagency_unreadable", &dispatch.id), content)
            .await;
    }
}

/// Same shape as the server's replies: top-level owner DMs stay flat.
fn reply_content(
    dispatch: &inbox::Dispatch,
    body: &str,
) -> Result<RoomMessageEventContent, RuntimeError> {
    let mut content = if body.len() <= MARKDOWN_BYTES {
        RoomMessageEventContent::text_markdown(body)
    } else if body.len() <= PLAIN_BYTES {
        RoomMessageEventContent::text_plain(body)
    } else {
        let mut end = PLAIN_BYTES;
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        RoomMessageEventContent::text_plain(format!("{}\n\n…（回复过长，已截断）", &body[..end]))
    };
    if dispatch.thread_root != dispatch.room_id {
        let root = OwnedEventId::try_from(dispatch.thread_root.as_str())
            .map_err(|_| RuntimeError::Request)?;
        content.relates_to = Some(Relation::Thread(Thread::plain(root.clone(), root)));
    }
    Ok(content)
}

fn request_text(decrypted: &DecryptedRoomEvent, requester: &str) -> Option<String> {
    // The Megolm session's Olm-authenticated sender, not just the envelope.
    if decrypted.encryption_info.sender.as_str() != requester {
        return None;
    }
    let Ok(AnyTimelineEvent::MessageLike(AnyMessageLikeEvent::RoomMessage(
        MessageLikeEvent::Original(message),
    ))) = decrypted.event.deserialize()
    else {
        return None;
    };
    if message.sender.as_str() != requester
        || matches!(message.content.relates_to, Some(Relation::Replacement(_)))
    {
        return None;
    }
    let MessageType::Text(text) = message.content.msgtype else {
        return None;
    };
    (!text.body.trim().is_empty() && text.body.len() <= 65_536).then_some(text.body)
}

async fn sync(client: Client, revoked: Arc<AtomicBool>) {
    loop {
        match client
            .sync_once(SyncSettings::default().timeout(Duration::from_secs(30)))
            .await
        {
            Ok(_) => {}
            Err(error) if unknown_token(&error) => {
                // Another grant replaced this device's token; the next request
                // obtains a fresh one under its current lease.
                revoked.store(true, Ordering::Release);
                return;
            }
            Err(_) => tokio::time::sleep(Duration::from_secs(5)).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dispatch(thread_root: &str) -> inbox::Dispatch {
        inbox::Dispatch {
            id: "evt_1".into(),
            binding_id: "bind_1".into(),
            agent_id: "agt_1".into(),
            event_id: "$request".into(),
            room_id: "!dm:example.test".into(),
            requester_mxid: "@owner:example.test".into(),
            thread_root: thread_root.into(),
            body: String::new(),
            state: "running".into(),
            binding_generation: 1,
            dispatch_epoch: Some(1),
            dispatch_device_id: Some("dev_1".into()),
            execution_id: Some("exe_1".into()),
            outcome: None,
        }
    }
    fn json(content: &RoomMessageEventContent) -> serde_json::Value {
        serde_json::to_value(content).unwrap()
    }

    #[test]
    fn encrypted_replies_keep_the_server_reply_shape() {
        let flat = json(&reply_content(&dispatch("!dm:example.test"), "**done**").unwrap());
        assert_eq!(flat["msgtype"], "m.text");
        assert_eq!(flat["body"], "**done**");
        assert_eq!(flat["format"], "org.matrix.custom.html");
        assert!(flat.get("m.relates_to").is_none());

        let threaded = json(&reply_content(&dispatch("$root"), "done").unwrap());
        assert_eq!(
            threaded["m.relates_to"],
            serde_json::json!({"rel_type":"m.thread","event_id":"$root",
                "is_falling_back":true,"m.in_reply_to":{"event_id":"$root"}})
        );
    }

    #[test]
    fn long_encrypted_replies_fit_one_matrix_event() {
        let plain =
            json(&reply_content(&dispatch("$root"), &"a".repeat(MARKDOWN_BYTES + 1)).unwrap());
        assert!(plain.get("formatted_body").is_none());
        let body = "答".repeat(PLAIN_BYTES);
        let cut = json(&reply_content(&dispatch("$root"), &body).unwrap());
        let text = cut["body"].as_str().unwrap();
        assert!(text.len() <= PLAIN_BYTES + 64 && text.ends_with("已截断）"));
        // Base64 Megolm ciphertext stays inside the 64 KiB event envelope.
        assert!(serde_json::to_vec(&cut).unwrap().len() * 4 / 3 < 62 * 1024);
    }

    #[test]
    fn reply_transaction_matches_the_server_receipt() {
        assert_eq!(
            txn("hagency", "evt_1").as_str(),
            format!("hagency_{:x}", Sha256::digest(b"evt_1"))
        );
        assert_ne!(txn("hagency_seen", "evt_1"), txn("hagency", "evt_1"));
    }

    #[test]
    fn store_passphrase_is_private_and_stable() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("agent");
        private_directory(&root).unwrap();
        let first = store_passphrase(&root).unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(store_passphrase(&root).unwrap(), first);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(root.join("store-key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0);
        }
    }
}
