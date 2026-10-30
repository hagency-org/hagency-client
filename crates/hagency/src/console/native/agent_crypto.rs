//! Host-only Agent crypto lifecycle. Identity readiness never grants message/tool authority.
use super::*;
use crate::console::{AuthorizedDevice, device_execution::DeviceIdentity, owned_agents};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};
use zeroize::Zeroizing;
#[cfg(all(test, not(any(target_os = "ios", target_os = "android"))))]
mod credential_tests;
#[cfg(any(target_os = "ios", target_os = "android", test))]
mod legacy;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod persistence;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AgentCryptoAction {
    Prepare { agent: String },
    Status { agent: String },
    Stop { agent: String },
    Verify { identity: AgentCryptoIdentity },
}
/// Public presentation only; this is not verification or execution authority.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCryptoIdentity {
    pub agent_id: String,
    pub agent_generation: i64,
    pub matrix_user_id: String,
    pub matrix_device_id: String,
    pub ed25519: String,
    pub curve25519: String,
    pub master_key: String,
}
/// Weak ownership prevents a managed SDK session keeping its NativeOwner alive.
#[derive(Clone)]
pub struct AgentCryptoAuthority {
    owner: Weak<Inner>,
    pin: DeviceIdentity,
    retired: Arc<AtomicBool>,
    publication: Arc<StdMutex<()>>,
}
impl AgentCryptoAuthority {
    pub fn retire(&self) {
        self.retired.store(true, Ordering::Release);
        drop(self.publication.try_lock());
    }
    async fn finish_retire(&self) -> crate::shutdown::Result {
        loop {
            match self.publication.try_lock() {
                Ok(_gate) => return Ok(()),
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    return Err(crate::shutdown::Error::Worker);
                }
                Err(std::sync::TryLockError::WouldBlock) => (),
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }
    pub fn is_retired(&self) -> bool {
        self.retired.load(Ordering::Acquire)
            || self
                .owner
                .upgrade()
                .is_none_or(|o| o.configuration_stopped.load(Ordering::Acquire))
    }
    pub async fn device(&self) -> Result<AuthorizedDevice, NativeError> {
        let owner = NativeOwner(
            self.owner
                .upgrade()
                .ok_or_else(|| failure(410, "native_owner_stopped"))?,
        );
        if self.is_retired() {
            return Err(failure(410, "native_owner_stopped"));
        }
        let device = owner.authorized_execution_device().await?;
        if DeviceIdentity::from(&device) != self.pin {
            return Err(failure(401, "agent_crypto_execution_device_changed"));
        }
        Ok(device)
    }
}
impl NativeOwner {
    pub async fn agent_crypto_authority(&self) -> Result<AgentCryptoAuthority, NativeError> {
        let device = self.authorized_execution_device().await?;
        Ok(AgentCryptoAuthority {
            owner: Arc::downgrade(&self.0),
            pin: DeviceIdentity::from(&device),
            retired: Arc::new(AtomicBool::new(false)),
            publication: self
                .0
                .host
                .console
                .0
                .server_login
                .credential_publication
                .clone(),
        })
    }
}
/// Private local initialization material; never Serialize, Clone or Debug.
pub struct AgentCryptoContext {
    pub(super) authority: AgentCryptoAuthority,
    agent: String,
    generation: i64,
    root: PathBuf,
    passphrase: Zeroizing<String>,
}
impl AgentCryptoContext {
    pub fn authority(&self) -> AgentCryptoAuthority {
        self.authority.clone()
    }
    pub fn agent(&self) -> &str {
        &self.agent
    }
    pub fn generation(&self) -> i64 {
        self.generation
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn passphrase(&self) -> &str {
        &self.passphrase
    }
}
pub type AgentCryptoPreparation<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<Arc<dyn AgentCryptoTransport>, NativeError>>
            + Send
            + 'a,
    >,
>;
/// Local crypto evidence only. Not a transport origin, membership or execution
/// grant. The original admitted queue event/lease must still be bound by runtime.
/// Deliberately not Clone, Debug, Serialize or Deserialize.
pub struct DecryptedAgentEvent {
    identity: AgentCryptoIdentity,
    room: String,
    event: String,
    sender: String,
    ciphertext_digest: String,
    plaintext: Value,
}
impl DecryptedAgentEvent {
    pub fn identity(&self) -> &AgentCryptoIdentity {
        &self.identity
    }
    pub fn room(&self) -> &str {
        &self.room
    }
    pub fn event(&self) -> &str {
        &self.event
    }
    pub fn sender(&self) -> &str {
        &self.sender
    }
    pub fn ciphertext_digest(&self) -> &str {
        &self.ciphertext_digest
    }
    pub fn into_plaintext(self) -> Value {
        self.plaintext
    }
}
pub trait AgentCryptoTransport: Send + Sync {
    fn receive_status(&self) -> Value {
        json!({"phase":"unsupported"})
    }
    fn decrypt_owner_event<'a>(
        &'a self,
        _room: &'a str,
        _event: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, NativeError>> + Send + 'a>>
    {
        Box::pin(async { Err(failure(503, "agent_crypto_receive_unavailable")) })
    }
    fn seal_reply<'a>(
        &'a self,
        _room: &'a str,
        _content: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, NativeError>> + Send + 'a>>
    {
        Box::pin(async { Err(failure(503, "agent_crypto_send_unavailable")) })
    }
    fn upload_encrypted_file<'a>(
        &'a self,
        _bytes: Vec<u8>,
        _live: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, NativeError>> + Send + 'a>>
    {
        Box::pin(async { Err(failure(503, "agent_crypto_send_unavailable")) })
    }
    fn identity(&self) -> AgentCryptoIdentity;
    fn retire(&self);
    fn refresh(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<AgentCryptoIdentity, NativeError>> + Send + '_>,
    >;
    fn close(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), NativeError>> + Send + '_>>;
}
/// Original AS delivery plus actual SDK decryption under the observed lease.
/// Not Clone/Serde: the payload and cryptographic receipt are host-owned evidence.
/// Admission/ACK/execution and subsequent current-scope checks remain separate.
pub struct DecryptedAgentCiphertext {
    source: crate::console::device_execution::Ciphertext,
    lease: crate::console::device_execution::LeaseRef,
    crypto: DecryptedAgentEvent,
}
impl DecryptedAgentCiphertext {
    pub fn source(&self) -> &crate::console::device_execution::Ciphertext {
        &self.source
    }
    pub fn lease(&self) -> &crate::console::device_execution::LeaseRef {
        &self.lease
    }
    pub fn into_plaintext(self) -> Value {
        self.crypto.into_plaintext()
    }
}
impl NativeOwner {
    async fn ciphertext_operation(
        &self,
        op: crate::console::device_execution::DeviceOperation,
        pin: &DeviceIdentity,
    ) -> Result<crate::console::device_execution::DeviceResponse, NativeError> {
        // Stop may hold `calls` while joining this original receive task.
        // SDK/source synchronization has its own lock; never take `calls` here.
        {
            self.sync_matrix().await?;
            self.check_matrix_source().await?;
        }
        self.0
            .host
            .console
            .execution_api_pinned(op, pin)
            .await
            .map_err(|e| failure(e.status, &e.code))
    }
    async fn ciphertext_mutation(
        &self,
        op: crate::console::device_execution::DeviceOperation,
        pin: &DeviceIdentity,
        authority: &AgentCryptoAuthority,
    ) -> Result<crate::console::device_execution::DeviceResponse, NativeError> {
        authority.device().await?;
        self.sync_matrix().await?;
        self.check_matrix_source().await?;
        authority.device().await?;
        let scope = authority.clone();
        self.0
            .host
            .console
            .execution_api_pinned_live(op, pin, Arc::new(move || !scope.is_retired()))
            .await
            .map_err(|e| failure(e.status, &e.code))
    }
    pub(in crate::console) async fn runtime_agent_crypto(
        &self,
        agent: &str,
    ) -> Result<Arc<dyn AgentCryptoTransport>, NativeError> {
        let device = self.authorized_execution_device().await?;
        let pin = DeviceIdentity::from(&device);
        let slots = self.0.crypto.slots.lock().map_err(|_| unavailable())?;
        let slot = slots
            .get(agent)
            .filter(|s| s.phase == "ready" && s.pin == pin && !s.authority.is_retired())
            .ok_or_else(|| failure(409, "agent_crypto_identity_changed"))?;
        slot.session.clone().ok_or_else(unavailable)
    }
    pub async fn poll_agent_ciphertexts(
        &self,
        lease: &crate::console::device_execution::LeaseRef,
        binding: &str,
        limit: usize,
    ) -> Result<Vec<crate::console::device_execution::Ciphertext>, NativeError> {
        let device = self.authorized_execution_device().await?;
        match self
            .ciphertext_operation(
                crate::console::device_execution::DeviceOperation::PollCiphertexts {
                    lease: lease.clone(),
                    binding_id: binding.into(),
                    limit,
                },
                &DeviceIdentity::from(&device),
            )
            .await?
        {
            crate::console::device_execution::DeviceResponse::Ciphertexts(events) => Ok(events),
            _ => Err(failure(502, "invalid_ciphertext_response")),
        }
    }
    /// Verify original AS/SDK input, persist its plaintext/source binding, then
    /// ACK only that original encrypted offer. No model/start permit is minted.
    pub async fn receive_agent_ciphertext(
        &self,
        lease: &crate::console::device_execution::LeaseRef,
        inbox_id: &str,
    ) -> Result<Value, NativeError> {
        use crate::console::device_execution::{
            DeviceOperation, DeviceResponse, encrypted::Projection,
        };
        key(&lease.agent_id)?;
        key(inbox_id)?;
        let update = crate::update_guard::PROCESS
            .enter_task()
            .ok_or_else(|| failure(409, "update_in_progress"))?;
        let device = self.authorized_execution_device().await?;
        let pin = DeviceIdentity::from(&device);
        let native = self.clone();
        let reference = lease.clone();
        let id = inbox_id.to_owned();
        let receiver = {
            let slots = self.0.crypto.slots.lock().map_err(|_| unavailable())?;
            let slot = slots
                .get(&lease.agent_id)
                .filter(|s| s.phase == "ready" && s.pin == pin && !s.authority.is_retired())
                .ok_or_else(|| failure(409, "agent_crypto_identity_changed"))?;
            let authority = slot.authority.clone();
            self.0.crypto.start_receive(&lease.agent_id,async move{
                let _update=update;
                authority.device().await?;
                let proof=native.decrypt_agent_ciphertext(&reference,&id).await?;
                let source=proof.source;let message=proof.crypto.into_plaintext();
                let projection=Projection::from_message(&message).map_err(|e|failure(e.status,&e.code))?;
                let DeviceResponse::EncryptedAdmission(prepared)=native.ciphertext_mutation(DeviceOperation::PrepareCiphertext{lease:reference.clone(),expected:source.clone(),projection},&pin,&authority).await? else{return Err(failure(502,"invalid_encrypted_admission"));};
                authority.device().await?;
                let local=prepared.local_dispatch(&message).map_err(|e|failure(e.status,&e.code))?;
                let root=native.0.host.console.0.server_login.state_directory().ok_or_else(||failure(503,"local_state_unavailable"))?;
                let path=owned_agents::ledger_path(&root,device.origin(),device.issuer(),device.subject(),device.owner_mxid()).map_err(|_|failure(503,"local_state_unavailable"))?;
                let profile=owned_agents::profile_identity(device.origin(),device.issuer(),device.subject(),device.owner_mxid()).map_err(|_|failure(401,"owner_authorization_required"))?;
                let mut ledger=hagency_agent_local::Ledger::open_scoped(&path,device.owner_mxid(),&profile).map_err(|_|failure(503,"local_state_unavailable"))?;
                ledger.register_binding(device.owner_mxid(),&local.scope()).map_err(|_|failure(409,"encrypted_input_binding_changed"))?;
                let input=hagency_agent_local::encrypted_inputs::Input{inbox_id:source.id.clone(),source_digest:source.source_digest.clone(),agent_generation:source.agent_generation,owner_direct:prepared.owner_direct,ciphertext:source.event.clone(),message};
                let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_|failure(503,"invalid_local_clock"))?.as_secs();
                let receipt=ledger.receive_encrypted_dispatch(device.owner_mxid(),&local,&input,hagency_agent_local::inbox::Limits::default(),i64::try_from(now).map_err(|_|failure(503,"invalid_local_clock"))?).map_err(|_|failure(409,"encrypted_input_persistence_failed"))?;
                let record=ledger.inbox_record(device.owner_mxid(),&local.id).map_err(|_|failure(503,"local_state_unavailable"))?;
                if !matches!(record.state,hagency_agent_local::inbox::State::Received|hagency_agent_local::inbox::State::Acknowledged){return Ok(json!({"inboxId":source.id,"dispatchId":local.id,"phase":"historical"}));}
                authority.device().await?;
                if prepared.dispatch.state!="encrypted_acknowledged"{
                    let DeviceResponse::EncryptedAdmission(_)=native.ciphertext_mutation(DeviceOperation::AckCiphertext{lease:reference,expected:source.clone(),prepared:Box::new(prepared)},&pin,&authority).await? else{return Err(failure(502,"invalid_encrypted_admission"));};
                }
                // Receipt recording is historical evidence, not fresh run custody.
                ledger.acknowledge_dispatch(device.owner_mxid(),&local.id).map_err(|_|failure(503,"encrypted_ack_record_failed"))?;
                Ok(json!({"inboxId":source.id,"dispatchId":local.id,"phase":"acknowledged","sourceDigest":source.source_digest,"alreadyDurable":receipt.already_durable}))
            })?
        };
        receiver
            .await
            .map_err(|_| failure(409, "agent_crypto_receive_unconfirmed"))?
    }
    pub async fn decrypt_agent_ciphertext(
        &self,
        lease: &crate::console::device_execution::LeaseRef,
        inbox_id: &str,
    ) -> Result<DecryptedAgentCiphertext, NativeError> {
        use crate::console::device_execution::{DeviceOperation, DeviceResponse};
        key(&lease.agent_id)?;
        key(inbox_id)?;
        let update = crate::update_guard::PROCESS
            .enter_task()
            .ok_or_else(|| failure(409, "update_in_progress"))?;
        let device = self.authorized_execution_device().await?;
        let pin = DeviceIdentity::from(&device);
        let native = self.clone();
        let id = inbox_id.to_owned();
        let reference = lease.clone();
        let receiver = {
            let slots = self.0.crypto.slots.lock().map_err(|_| unavailable())?;
            let slot = slots
                .get(&lease.agent_id)
                .filter(|s| s.phase == "ready" && s.pin == pin && !s.authority.is_retired())
                .ok_or_else(|| failure(409, "agent_crypto_identity_changed"))?;
            let session = slot.session.as_ref().ok_or_else(unavailable)?.clone();
            let identity = session.identity();
            let authority = slot.authority.clone();
            self.0.crypto.start_receive(&lease.agent_id, async move {
                let _update = update;
                authority.device().await?;
                let read = || DeviceOperation::ReadCiphertext {
                    lease: reference.clone(),
                    inbox_id: id.clone(),
                };
                let DeviceResponse::Ciphertext(source) =
                    native.ciphertext_operation(read(), &pin).await?
                else {
                    return Err(failure(502, "invalid_ciphertext_response"));
                };
                if source.agent_id != identity.agent_id
                    || source.agent_generation != identity.agent_generation
                {
                    return Err(failure(409, "agent_crypto_identity_changed"));
                }
                let owner = device.owner_mxid();
                if source.sender_mxid != owner {
                    return Err(failure(403, "agent_crypto_owner_sender_required"));
                }
                let plaintext = session
                    .decrypt_owner_event(&source.room_id, &source.event)
                    .await?;
                authority.device().await?;
                if plaintext["event_id"] != source.event_id
                    || plaintext["sender"] != owner
                    || plaintext
                        .get("room_id")
                        .is_some_and(|r| r != &source.room_id)
                    || plaintext["type"] != "m.room.message"
                    || plaintext["unsigned"].get("redacted_because").is_some()
                {
                    return Err(failure(409, "agent_crypto_invalid_event"));
                }
                use sha2::{Digest, Sha256};
                let ciphertext_digest = format!(
                    "{:x}",
                    Sha256::digest(
                        serde_json::to_vec(&source.event)
                            .map_err(|_| failure(502, "invalid_ciphertext_response"))?
                    )
                );
                let crypto = DecryptedAgentEvent {
                    identity,
                    room: source.room_id.clone(),
                    event: source.event_id.clone(),
                    sender: owner.into(),
                    ciphertext_digest,
                    plaintext,
                };
                // A real fresh read after crypto checks the original AS commitment
                // and current membership/binding/generation/device/lease again.
                let DeviceResponse::Ciphertext(current) =
                    native.ciphertext_operation(read(), &pin).await?
                else {
                    return Err(failure(502, "invalid_ciphertext_response"));
                };
                if current != source {
                    return Err(failure(409, "ciphertext_changed"));
                }
                authority.device().await?;
                Ok(DecryptedAgentCiphertext {
                    source,
                    lease: reference,
                    crypto,
                })
            })?
        };
        receiver
            .await
            .map_err(|_| failure(409, "agent_crypto_receive_unconfirmed"))?
    }
}

impl NativeOwner {
    /// Decrypt an already received owner event with the original host SDK.
    /// This does not admit a task, ACK an event, obtain a lease or start a model.
    /// Dropping a caller cannot discard original crypto IO or its shutdown join.
    pub async fn decrypt_agent_owner_event(
        &self,
        agent: &str,
        generation: i64,
        room: &str,
        event: Value,
    ) -> Result<DecryptedAgentEvent, NativeError> {
        key(agent)?;
        if generation <= 0
            || !crate::console::matrix_identifiers::room_id(room)
            || event["type"] != "m.room.encrypted"
            || event["room_id"] != room
            || event["event_id"].as_str().is_none_or(|id| {
                !id.starts_with('$') || id.len() > 1024 || id.chars().any(char::is_control)
            })
        {
            return Err(failure(400, "agent_crypto_invalid_event"));
        }
        let bytes =
            serde_json::to_vec(&event).map_err(|_| failure(400, "agent_crypto_invalid_event"))?;
        if bytes.len() > 131072 {
            return Err(failure(400, "agent_crypto_invalid_event"));
        }
        let update = crate::update_guard::PROCESS
            .enter_task()
            .ok_or_else(|| failure(409, "update_in_progress"))?;
        {
            let _call = self.0.calls.lock().await;
            self.sync_matrix().await?;
            self.check_matrix_source().await?;
        }
        let device = self.authorized_execution_device().await?;
        if event["sender"] != device.owner_mxid() {
            return Err(failure(403, "agent_crypto_owner_sender_required"));
        }
        let (session, authority) = {
            let slots = self.0.crypto.slots.lock().map_err(|_| unavailable())?;
            let slot = slots
                .get(agent)
                .filter(|s| {
                    s.phase == "ready"
                        && s.generation == generation
                        && s.pin == DeviceIdentity::from(&device)
                        && !s.authority.is_retired()
                })
                .ok_or_else(|| failure(409, "agent_crypto_identity_changed"))?;
            (
                slot.session.clone().ok_or_else(unavailable)?,
                slot.authority.clone(),
            )
        };
        let room = room.to_owned();
        let event_id = event["event_id"].as_str().unwrap().to_owned();
        let sender = device.owner_mxid().to_owned();
        let identity = session.identity();
        use sha2::{Digest, Sha256};
        let ciphertext_digest = format!("{:x}", Sha256::digest(&bytes));
        let receive = {
            // Serialize admission with stop's retirement and task snapshot.
            let _slots = self.0.crypto.slots.lock().map_err(|_| unavailable())?;
            if authority.is_retired() {
                return Err(failure(410, "native_owner_stopped"));
            }
            self.0.crypto.start_receive(agent, async move {
                let _update = update;
                let plaintext = session.decrypt_owner_event(&room, &event).await?;
                authority.device().await?;
                if plaintext["event_id"] != event_id
                    || plaintext["sender"] != sender
                    || plaintext.get("room_id").is_some_and(|r| r != &room)
                    || plaintext["type"] != "m.room.message"
                    || plaintext["unsigned"].get("redacted_because").is_some()
                {
                    return Err(failure(409, "agent_crypto_invalid_event"));
                }
                Ok(DecryptedAgentEvent {
                    identity,
                    room,
                    event: event_id,
                    sender,
                    ciphertext_digest,
                    plaintext,
                })
            })
        }?;
        receive
            .await
            .map_err(|_| failure(409, "agent_crypto_receive_unconfirmed"))?
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Credential {
    version: u8,
    profile: String,
    agent: String,
    device: String,
    passphrase: String,
}
impl Drop for Credential {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.passphrase.zeroize();
    }
}
fn unavailable() -> NativeError {
    failure(409, "agent_crypto_local_state_unavailable")
}
async fn material(
    native: &NativeOwner,
    d: &AuthorizedDevice,
    agent: &str,
    generation: i64,
    authority: AgentCryptoAuthority,
) -> Result<AgentCryptoContext, NativeError> {
    let root = native
        .0
        .host
        .console
        .0
        .server_login
        .state_directory()
        .ok_or_else(unavailable)?;
    let device = d.clone();
    let agent = agent.to_owned();
    native
        .0
        .host
        .console
        .0
        .server_login
        .credential_io
        .call(move |vault| {
            #[cfg(any(target_os = "ios", target_os = "android"))]
            {
                let _ = vault;
                legacy::load(root, device, agent, generation, authority)
            }
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            {
                persistence::load(vault, root, device, agent, generation, authority)
            }
        })
        .await
        .map_err(|error| match error {
            hagency_store::credential_vault::Error::Missing => {
                failure(409, "agent_crypto_original_secret_missing")
            }
            hagency_store::credential_vault::Error::Retired => failure(410, "native_owner_stopped"),
            _ => unavailable(),
        })
}

static STARTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
#[derive(Default)]
pub(super) struct Registry {
    closed: AtomicBool,
    slots: StdMutex<BTreeMap<String, Slot>>,
    receives: StdMutex<Vec<(String, crate::shutdown::Task)>>,
}
struct Slot {
    authority: AgentCryptoAuthority,
    pin: DeviceIdentity,
    generation: i64,
    phase: &'static str,
    error: Option<String>,
    session: Option<Arc<dyn AgentCryptoTransport>>,
    init: Option<crate::shutdown::Task>,
    closing: Option<crate::shutdown::Task>,
    verification: Option<crate::shutdown::Task>,
    owner_verified: bool,
    trust_error: Option<String>,
}
fn view(slot: Option<&Slot>) -> Value {
    slot.map_or_else(||json!({"phase":"not_initialized"}),|s|json!({"phase":s.phase,"agentGeneration":s.generation,"error":s.error,"identity":s.session.as_ref().map(|h|h.identity()),"ownerVerified":s.phase=="ready" && s.owner_verified && !s.authority.is_retired(),"verificationPending":s.verification.as_ref().is_some_and(|t|t.result().is_none()),"verificationError":s.trust_error,"receive":s.session.as_ref().map(|h|h.receive_status())}))
}
impl Registry {
    fn start_receive<T: Send + 'static>(
        &self,
        agent: &str,
        job: impl std::future::Future<Output = Result<T, NativeError>> + Send + 'static,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<T, NativeError>>, NativeError> {
        let (send, receive) = tokio::sync::oneshot::channel();
        let mut tasks = self.receives.lock().map_err(|_| unavailable())?;
        tasks.retain(|(_, task)| task.result() != Some(Ok(())));
        if self.closed.load(Ordering::Acquire) || tasks.len() >= 32 {
            return Err(failure(409, "agent_crypto_receive_unavailable"));
        }
        tasks.push((
            agent.into(),
            crate::shutdown::Task::start(async move {
                let result = job.await;
                let _ = send.send(result);
                Ok(())
            }),
        ));
        Ok(receive)
    }
    pub(super) fn retire(&self) {
        self.closed.store(true, Ordering::Release);
        if let Ok(mut slots) = self.slots.lock() {
            for s in slots.values_mut() {
                s.authority.retire();
                s.phase = "stopping";
                if let Some(h) = &s.session {
                    h.retire();
                }
            }
        }
    }
    pub(super) async fn shutdown(&self) -> crate::shutdown::Result {
        self.retire();
        let tasks = self
            .slots
            .lock()
            .map_err(|_| crate::shutdown::Error::Worker)?
            .values()
            .flat_map(|s| {
                [s.init.clone(), s.verification.clone(), s.closing.clone()]
                    .into_iter()
                    .flatten()
            })
            .collect::<Vec<_>>();
        let receives = self
            .receives
            .lock()
            .map_err(|_| crate::shutdown::Error::Worker)?
            .clone();
        let authorities = self
            .slots
            .lock()
            .map_err(|_| crate::shutdown::Error::Worker)?
            .values()
            .map(|s| s.authority.clone())
            .collect::<Vec<_>>();
        let mut result = Ok(());
        for authority in authorities {
            result = result.and(authority.finish_retire().await);
        }
        for (_, task) in receives {
            result = result.and(task.wait().await);
        }
        for task in tasks {
            result = result.and(task.wait().await);
        }
        let sessions = self
            .slots
            .lock()
            .map_err(|_| crate::shutdown::Error::Worker)?
            .values()
            .filter_map(|s| s.session.clone())
            .collect::<Vec<_>>();
        for session in sessions {
            if session.close().await.is_err() {
                result = Err(crate::shutdown::Error::Worker);
            }
        }
        if result.is_ok()
            && let Ok(mut slots) = self.slots.lock()
        {
            for s in slots.values_mut() {
                s.session = None;
                s.phase = "stopped";
            }
        }
        result
    }
}
fn error_code(error: NativeError) -> String {
    if error.code.starts_with("agent_crypto_")
        && error.code.len() < 100
        && error
            .code
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c == b'_')
    {
        error.code
    } else {
        "agent_crypto_prepare_failed".into()
    }
}
async fn stop(native: &NativeOwner, agent: &str) -> Result<(), NativeError> {
    let closing = {
        let mut slots = native.0.crypto.slots.lock().map_err(|_| unavailable())?;
        let Some(slot) = slots.get_mut(agent) else {
            return Ok(());
        };
        slot.authority.retire();
        if let Some(session) = &slot.session {
            session.retire();
        }
        if slot.closing.is_none() {
            slot.phase = "stopping";
            let authority = slot.authority.clone();
            let init = slot.init.clone();
            let verification = slot.verification.clone();
            let receives = native
                .0
                .crypto
                .receives
                .lock()
                .map_err(|_| unavailable())?
                .iter()
                .filter(|(id, _)| id == agent)
                .map(|(_, task)| task.clone())
                .collect::<Vec<_>>();
            let session = slot.session.clone();
            let weak = Arc::downgrade(&native.0);
            let id = agent.to_owned();
            let update = crate::update_guard::PROCESS
                .continue_task()
                .ok_or_else(|| failure(409, "update_in_progress"))?;
            slot.closing = Some(crate::shutdown::Task::start(async move {
                let _update = update;
                let retired = authority.finish_retire().await;
                let mut result = if let Some(init) = init {
                    init.wait().await
                } else {
                    Ok(())
                };
                result = result.and(retired);
                if let Some(verification) = verification {
                    result = result.and(verification.wait().await);
                }
                for receive in receives {
                    result = result.and(receive.wait().await);
                }
                if let Some(session) = session
                    && session.close().await.is_err()
                {
                    result = Err(crate::shutdown::Error::Worker);
                }
                if let Some(owner) = weak.upgrade()
                    && let Ok(mut slots) = owner.crypto.slots.lock()
                    && let Some(slot) = slots.get_mut(&id)
                {
                    if result.is_ok() {
                        slot.session = None;
                        slot.phase = "stopped";
                    } else {
                        slot.phase = "close_unconfirmed";
                        slot.error = Some("agent_crypto_shutdown_failed".into());
                    }
                }
                result
            }));
        }
        slot.closing.as_ref().unwrap().clone()
    };
    closing
        .wait()
        .await
        .map_err(|_| failure(409, "agent_crypto_shutdown_failed"))
}
async fn refresh_trust(native: &NativeOwner, agent: &str) {
    let prepared = native.0.crypto.slots.lock().ok().and_then(|slots| {
        let slot = slots.get(agent)?;
        (slot.phase == "ready"
            && !slot.authority.is_retired()
            && !slot
                .verification
                .as_ref()
                .is_some_and(|t| t.result().is_none()))
        .then(|| {
            slot.session
                .as_ref()
                .map(|session| (session.identity(), slot.authority.clone()))
        })
        .flatten()
    });
    let Some((identity, authority)) = prepared else {
        return;
    };
    let source = native.0.matrix_source.lock().ok().and_then(|s| s.clone());
    let Some(source) = source else {
        return;
    };
    let trust = source
        .agent_crypto_owner_trust(identity.clone(), authority, false)
        .await;
    if let Ok(mut slots) = native.0.crypto.slots.lock()
        && let Some(slot) = slots.get_mut(agent).filter(|s| {
            !s.authority.is_retired()
                && s.session.as_ref().is_some_and(|h| h.identity() == identity)
        })
    {
        slot.owner_verified = matches!(trust, Ok(true));
        if let Err(e) = trust {
            slot.trust_error = Some(error_code(e));
        } else {
            slot.trust_error = None;
        }
    }
}
async fn verify(
    native: &NativeOwner,
    expected: &AgentCryptoIdentity,
    pin: &DeviceIdentity,
) -> Result<Value, NativeError> {
    let source = native
        .0
        .matrix_source
        .lock()
        .map_err(|_| unavailable())?
        .clone()
        .ok_or_else(|| failure(503, "agent_crypto_owner_verification_unavailable"))?;
    let mut slots = native.0.crypto.slots.lock().map_err(|_| unavailable())?;
    let slot = slots
        .get_mut(&expected.agent_id)
        .filter(|s| {
            s.phase == "ready"
                && s.pin == *pin
                && !s.authority.is_retired()
                && s.session
                    .as_ref()
                    .is_some_and(|h| h.identity() == *expected)
        })
        .ok_or_else(|| failure(409, "agent_crypto_identity_changed"))?;
    if slot
        .verification
        .as_ref()
        .is_some_and(|t| t.result().is_none())
    {
        return Ok(json!({"agentId":expected.agent_id,"crypto":view(Some(slot))}));
    }
    let session = slot.session.as_ref().unwrap().clone();
    let authority = slot.authority.clone();
    let identity = expected.clone();
    let weak = Arc::downgrade(&native.0);
    let id = expected.agent_id.clone();
    let update = crate::update_guard::PROCESS
        .continue_task()
        .ok_or_else(|| failure(409, "update_in_progress"))?;
    slot.owner_verified = false;
    slot.trust_error = None;
    slot.verification = Some(crate::shutdown::Task::start(async move {
        let _update = update;
        let result = async {
            if session.refresh().await? != identity {
                return Err(failure(409, "agent_crypto_identity_changed"));
            }
            source
                .agent_crypto_owner_trust(identity.clone(), authority, true)
                .await
        }
        .await;
        if let Some(owner) = weak.upgrade()
            && let Ok(mut slots) = owner.crypto.slots.lock()
            && let Some(slot) = slots
                .get_mut(&id)
                .filter(|s| s.session.as_ref().is_some_and(|h| h.identity() == identity))
        {
            slot.owner_verified = matches!(result, Ok(true)) && !slot.authority.is_retired();
            slot.trust_error = result.err().map(error_code);
        }
        Ok(())
    }));
    Ok(json!({"agentId":expected.agent_id,"crypto":view(Some(slot))}))
}

pub(super) async fn execute(
    native: &NativeOwner,
    action: AgentCryptoAction,
) -> Result<Value, NativeError> {
    let agent = match &action {
        AgentCryptoAction::Prepare { agent }
        | AgentCryptoAction::Status { agent }
        | AgentCryptoAction::Stop { agent } => agent,
        AgentCryptoAction::Verify { identity } => &identity.agent_id,
    };
    key(agent)?;
    let d = native.authorized_execution_device().await?;
    let pin = DeviceIdentity::from(&d);
    let registry = &native.0.crypto;
    if let AgentCryptoAction::Verify { identity } = &action {
        return verify(native, identity, &pin).await;
    }
    if matches!(action, AgentCryptoAction::Status { .. }) {
        let session = {
            let mut slots = registry.slots.lock().map_err(|_| unavailable())?;
            if let Some(s) = slots.get_mut(agent) {
                if s.pin != pin {
                    s.authority.retire();
                    if let Some(h) = &s.session {
                        h.retire();
                    }
                    s.phase = "stale";
                    s.error = Some("agent_crypto_execution_device_changed".into());
                }
                if s.phase == "initializing"
                    && s.init.as_ref().is_some_and(|t| t.result().is_some())
                {
                    s.phase = "failed";
                    s.error = Some("agent_crypto_initialization_interrupted".into());
                }
                if matches!(s.phase, "ready" | "unavailable") {
                    s.session.clone()
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(session) = session {
            let result = session.refresh().await;
            let mut slots = registry.slots.lock().map_err(|_| unavailable())?;
            if let Some(s) = slots.get_mut(agent).filter(|s| !s.authority.is_retired()) {
                match result {
                    Ok(_) => {
                        s.phase = "ready";
                        s.error = None;
                    }
                    Err(e) => {
                        s.phase = "unavailable";
                        s.error = Some(error_code(e));
                    }
                }
            }
        }
        refresh_trust(native, agent).await;
        let slots = registry.slots.lock().map_err(|_| unavailable())?;
        return Ok(json!({"agentId":agent,"crypto":view(slots.get(agent))}));
    }
    if matches!(action, AgentCryptoAction::Prepare { .. }) {
        let slots = registry.slots.lock().map_err(|_| unavailable())?;
        if slots
            .get(agent)
            .is_some_and(|s| s.phase == "initializing" && s.pin == pin)
        {
            return Ok(json!({"agentId":agent,"crypto":view(slots.get(agent))}));
        }
    }
    stop(native, agent).await?;
    if matches!(action, AgentCryptoAction::Stop { .. }) {
        return Ok(json!({"agentId":agent,"crypto":{"phase":"stopped"}}));
    }
    let source = native
        .0
        .matrix_source
        .lock()
        .map_err(|_| unavailable())?
        .clone()
        .filter(|s| s.supports_agent_crypto())
        .ok_or_else(|| failure(503, "agent_crypto_transport_unavailable"))?;
    let cookie = native.0.cookie.lock().await.clone();
    let (method, path, body) = request(Command::AgentDetails {
        agent_id: agent.to_string(),
    })?;
    let mut response = native.call(method, &path, body, &cookie).await;
    let details = read_response(&mut response).await?;
    let a = &details["agent"];
    let generation = a["generation"]
        .as_i64()
        .filter(|g| *g > 0)
        .ok_or_else(|| failure(502, "invalid_server_response"))?;
    if a["id"] != agent.as_str()
        || a["executionDeviceId"] != d.device_id()
        || !matches!(
            a["state"].as_str(),
            Some("creating" | "active" | "suspended")
        )
    {
        return Err(failure(409, "agent_execution_device_required"));
    }
    let expected_user = a["puppetMxid"]
        .as_str()
        .filter(|u| u.starts_with("@_hagency_") && u.len() <= 255 && u.contains(':'))
        .ok_or_else(|| failure(502, "invalid_server_response"))?
        .to_owned();
    let start = STARTS
        .try_acquire()
        .map_err(|_| failure(409, "agent_crypto_busy"))?;
    {
        let slots = registry.slots.lock().map_err(|_| unavailable())?;
        if registry.closed.load(Ordering::Acquire)
            || (!slots.contains_key(agent) && slots.len() >= 128)
            || slots
                .values()
                .filter(|s| {
                    s.session.is_some()
                        || s.init.as_ref().is_some_and(|task| task.result().is_none())
                })
                .count()
                >= 32
        {
            return Err(failure(409, "agent_crypto_unavailable"));
        }
    }
    let authority = AgentCryptoAuthority {
        owner: Arc::downgrade(&native.0),
        pin: pin.clone(),
        retired: Arc::new(AtomicBool::new(false)),
        publication: native
            .0
            .host
            .console
            .0
            .server_login
            .credential_publication
            .clone(),
    };
    let task_owner = native.clone();
    let task_authority = authority.clone();
    let (launch, launched) = tokio::sync::oneshot::channel();
    let update = crate::update_guard::PROCESS
        .continue_task()
        .ok_or_else(|| failure(409, "update_in_progress"))?;
    let weak = Arc::downgrade(&native.0);
    let id = agent.to_string();
    let mut slots = registry.slots.lock().map_err(|_| unavailable())?;
    if registry.closed.load(Ordering::Acquire) {
        return Err(failure(410, "native_owner_stopped"));
    }
    let task = crate::shutdown::Task::start(async move {
        let _update = update;
        let _start = start;
        if launched.await.is_err() {
            return Err(crate::shutdown::Error::Worker);
        }
        let result = match material(&task_owner, &d, &id, generation, task_authority).await {
            Ok(context) => source.prepare_agent_crypto(context).await,
            Err(error) => Err(error),
        };
        drop(task_owner);
        let session = match result {
            Ok(session) => session,
            Err(e) => {
                if let Some(owner) = weak.upgrade()
                    && let Ok(mut slots) = owner.crypto.slots.lock()
                    && let Some(s) = slots.get_mut(&id)
                {
                    s.phase = if s.authority.is_retired() {
                        "stopped"
                    } else {
                        "failed"
                    };
                    s.error = Some(error_code(e));
                }
                return Ok(());
            }
        };
        let identity = session.identity();
        let valid = identity.agent_id == id
            && identity.agent_generation == generation
            && identity.matrix_user_id == expected_user
            && identity.ed25519.len() == 43
            && identity.curve25519.len() == 43
            && identity.master_key.len() == 43;
        let accepted = if let Some(owner) = weak.upgrade() {
            if let Ok(mut slots) = owner.crypto.slots.lock() {
                if let Some(s) = slots.get_mut(&id).filter(|s| {
                    valid
                        && !owner.crypto.closed.load(Ordering::Acquire)
                        && !s.authority.is_retired()
                }) {
                    s.phase = "ready";
                    s.session = Some(session.clone());
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };
        if !accepted {
            session.retire();
            session
                .close()
                .await
                .map_err(|_| crate::shutdown::Error::Worker)?;
            if let Some(owner) = weak.upgrade()
                && let Ok(mut slots) = owner.crypto.slots.lock()
                && let Some(s) = slots.get_mut(&id)
            {
                s.phase = if s.authority.is_retired() {
                    "stopped"
                } else {
                    "failed"
                };
                s.error = Some("agent_crypto_identity_mismatch".into());
            }
        }
        Ok(())
    });
    slots.insert(
        agent.to_string(),
        Slot {
            authority,
            pin,
            generation,
            phase: "initializing",
            error: None,
            session: None,
            init: Some(task),
            closing: None,
            verification: None,
            owner_verified: false,
            trust_error: None,
        },
    );
    let _ = launch.send(());
    Ok(json!({"agentId":agent,"crypto":view(slots.get(agent))}))
}
/// Normal create/start/assignment actions initialize identities on this device;
/// stopping or retiring an Agent joins its original identity job.
pub(super) async fn automatic(native: &NativeOwner, command: &Command, result: &mut Value) {
    if !result.is_object()
        || !native
            .0
            .matrix_source
            .lock()
            .is_ok_and(|s| s.as_ref().is_some_and(|s| s.supports_agent_crypto()))
    {
        return;
    }
    let (agent, prepare) = match command {
        Command::CreateAgent { .. } => match result["creation"]["agent"]["id"].as_str() {
            Some(id) => (id.to_owned(), true),
            None => return,
        },
        Command::AssignAgentToCurrentDevice { agent_id, .. } => (agent_id.clone(), true),
        Command::Agent { agent_id, action } => {
            (agent_id.clone(), matches!(action, AgentAction::Resume))
        }
        Command::Local {
            agent_id,
            action: LocalAction::RuntimeStart | LocalAction::StartAgentService,
            ..
        } => (agent_id.clone(), true),
        Command::Local {
            agent_id,
            action: LocalAction::StopAgentService,
            ..
        } => (agent_id.clone(), false),
        _ => return,
    };
    let action = if prepare {
        AgentCryptoAction::Prepare { agent }
    } else {
        AgentCryptoAction::Stop { agent }
    };
    result["crypto"] = match execute(native, action).await {
        Ok(value) => value["crypto"].clone(),
        Err(e) => json!({"phase":"unavailable","error":error_code(e)}),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    async fn owner(root: &Path) -> NativeOwner {
        NativeOwner::open_inner_with_credentials(
            root,
            "http://127.0.0.1:9",
            "@owner:example.test",
            false,
            crate::console::server_login::credential_tests::vault(),
        )
        .await
        .unwrap()
    }
    fn device() -> AuthorizedDevice {
        crate::console::server_login::fixture_device(
            "http://127.0.0.1:9/",
            "owner",
            "@owner:example.test",
        )
        .0
    }
    fn authority(owner: &NativeOwner, d: &AuthorizedDevice) -> AgentCryptoAuthority {
        AgentCryptoAuthority {
            owner: Arc::downgrade(&owner.0),
            pin: DeviceIdentity::from(d),
            retired: Arc::new(AtomicBool::new(false)),
            publication: owner
                .0
                .host
                .console
                .0
                .server_login
                .credential_publication
                .clone(),
        }
    }
    #[tokio::test]
    async fn agent_crypto_private_material_is_scoped_and_retains_original_keys_across_epochs() {
        let root = tempfile::tempdir().unwrap();
        let owner = owner(&root.path().join("state")).await;
        let d = device();
        let a = material(&owner, &d, "agent-a", 1, authority(&owner, &d))
            .await
            .unwrap();
        let again = material(&owner, &d, "agent-a", 2, authority(&owner, &d))
            .await
            .unwrap();
        assert_eq!(a.root, again.root);
        assert_eq!(a.passphrase, again.passphrase);
        let b = material(&owner, &d, "agent-b", 1, authority(&owner, &d))
            .await
            .unwrap();
        assert_ne!(a.root, b.root);
        assert_ne!(a.passphrase, b.passphrase);
        let other = crate::console::server_login::fixture_device(
            "http://127.0.0.1:9/",
            "other",
            "@other:example.test",
        )
        .0;
        let c = material(&owner, &other, "agent-a", 1, authority(&owner, &other))
            .await
            .unwrap();
        assert_ne!(a.root, c.root);
        assert_ne!(a.passphrase, c.passphrase);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(a.root.parent().unwrap().join("credential.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&a.root).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        owner.shutdown_checked().await.unwrap();
    }
    #[tokio::test]
    async fn agent_crypto_missing_or_misbound_original_secret_cannot_generate_replacement() {
        let root = tempfile::tempdir().unwrap();
        let owner = owner(&root.path().join("state")).await;
        let d = device();
        let original = material(&owner, &d, "agent-a", 1, authority(&owner, &d))
            .await
            .unwrap();
        let path = original.root.parent().unwrap().join("credential.json");
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            material(&owner, &d, "agent-a", 1, authority(&owner, &d))
                .await
                .err()
                .unwrap()
                .code,
            "agent_crypto_original_secret_missing"
        );
        assert!(!path.exists());
        let mut wrong: Value = serde_json::from_slice(&bytes).unwrap();
        wrong["agent"] = json!("other");
        hagency_store::private::write_new(&path, &serde_json::to_vec(&wrong).unwrap()).unwrap();
        assert_eq!(
            material(&owner, &d, "agent-a", 1, authority(&owner, &d))
                .await
                .err()
                .unwrap()
                .code,
            "agent_crypto_local_state_unavailable"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            serde_json::to_vec(&wrong).unwrap()
        );
        owner.shutdown_checked().await.unwrap();
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn agent_crypto_symlink_secret_cannot_redirect_original_credentials() {
        let root = tempfile::tempdir().unwrap();
        let owner = owner(&root.path().join("state")).await;
        let d = device();
        let original = material(&owner, &d, "agent-a", 1, authority(&owner, &d))
            .await
            .unwrap();
        let path = original.root.parent().unwrap().join("credential.json");
        let target = root.path().join("untouched");
        std::fs::write(&target, b"original").unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(
            material(&owner, &d, "agent-a", 1, authority(&owner, &d))
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(target).unwrap(), b"original");
        owner.shutdown_checked().await.unwrap();
    }
    struct HeldTransport {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        retired: AtomicBool,
        closed: AtomicBool,
    }
    impl AgentCryptoTransport for HeldTransport {
        fn identity(&self) -> AgentCryptoIdentity {
            AgentCryptoIdentity {
                agent_id: "agent-a".into(),
                agent_generation: 1,
                matrix_user_id: "@_hagency_a:example.test".into(),
                matrix_device_id: "public".into(),
                ed25519: "public".into(),
                curve25519: "public".into(),
                master_key: "public".into(),
            }
        }
        fn retire(&self) {
            self.retired.store(true, Ordering::Release);
        }
        fn refresh(
            &self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<AgentCryptoIdentity, NativeError>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async { Ok(self.identity()) })
        }
        fn close(
            &self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), NativeError>> + Send + '_>>
        {
            Box::pin(async {
                if !self.closed.load(Ordering::Acquire) {
                    self.entered.notify_one();
                    self.release.notified().await;
                    self.closed.store(true, Ordering::Release);
                }
                Ok(())
            })
        }
    }
    #[tokio::test]
    async fn agent_crypto_canceled_stop_waiter_keeps_original_cleanup_joined_by_shutdown() {
        let root = tempfile::tempdir().unwrap();
        let owner = owner(&root.path().join("state")).await;
        let d = device();
        let transport = Arc::new(HeldTransport {
            entered: Default::default(),
            release: Default::default(),
            retired: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        });
        owner.0.crypto.slots.lock().unwrap().insert(
            "agent-a".into(),
            Slot {
                authority: authority(&owner, &d),
                pin: DeviceIdentity::from(&d),
                generation: 1,
                phase: "ready",
                error: None,
                session: Some(transport.clone()),
                init: None,
                closing: None,
                verification: None,
                owner_verified: false,
                trust_error: None,
            },
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(10), stop(&owner, "agent-a"))
                .await
                .is_err()
        );
        tokio::time::timeout(Duration::from_secs(1), transport.entered.notified())
            .await
            .unwrap();
        assert!(transport.retired.load(Ordering::Acquire));
        assert!(
            owner.0.crypto.slots.lock().unwrap()["agent-a"]
                .closing
                .is_some()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(10), owner.0.crypto.shutdown())
                .await
                .is_err()
        );
        assert!(!transport.closed.load(Ordering::Acquire));
        transport.release.notify_one();
        owner.0.crypto.shutdown().await.unwrap();
        assert!(transport.closed.load(Ordering::Acquire));
        stop(&owner, "agent-a").await.unwrap();
        assert_eq!(
            owner.0.crypto.slots.lock().unwrap()["agent-a"].phase,
            "stopped"
        );
        assert!(
            owner.0.crypto.slots.lock().unwrap()["agent-a"]
                .session
                .is_none()
        );
        owner.shutdown_checked().await.unwrap();
    }
    #[tokio::test]
    async fn agent_crypto_shutdown_retains_initialization_receipt_after_waiter_cancellation() {
        let root = tempfile::tempdir().unwrap();
        let owner = owner(&root.path().join("state")).await;
        let d = device();
        let (release, held) = tokio::sync::oneshot::channel();
        let init = crate::shutdown::Task::start(async {
            held.await.map_err(|_| crate::shutdown::Error::Worker)?;
            Ok(())
        });
        owner.0.crypto.slots.lock().unwrap().insert(
            "agent-a".into(),
            Slot {
                authority: authority(&owner, &d),
                pin: DeviceIdentity::from(&d),
                generation: 1,
                phase: "initializing",
                error: None,
                session: None,
                init: Some(init.clone()),
                closing: None,
                verification: None,
                owner_verified: false,
                trust_error: None,
            },
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(10), stop(&owner, "agent-a"))
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(10), owner.0.crypto.shutdown())
                .await
                .is_err()
        );
        assert!(
            owner.0.crypto.slots.lock().unwrap()["agent-a"]
                .authority
                .is_retired()
        );
        assert_eq!(init.result(), None);
        release.send(()).unwrap();
        owner.0.crypto.shutdown().await.unwrap();
        assert_eq!(init.result(), Some(Ok(())));
        owner.shutdown_checked().await.unwrap();
    }
    struct HeldTrust {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        writes: std::sync::atomic::AtomicUsize,
    }
    impl MatrixTokenSource for HeldTrust {
        fn access_token(
            &self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async { Err(failure(401, "fixture_unused")) })
        }
        fn agent_crypto_owner_trust(
            &self,
            _identity: AgentCryptoIdentity,
            _authority: AgentCryptoAuthority,
            verify: bool,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<bool, NativeError>> + Send + '_>,
        > {
            Box::pin(async move {
                if verify {
                    self.writes.fetch_add(1, Ordering::AcqRel);
                    self.entered.notify_one();
                    self.release.notified().await;
                }
                // Deliberately return a late positive host-double observation;
                // it must not restore the retired native session's state.
                Ok(true)
            })
        }
    }
    #[tokio::test]
    async fn agent_crypto_exact_verification_rejects_changed_keys_and_joins_original_on_stop() {
        let root = tempfile::tempdir().unwrap();
        let native = owner(&root.path().join("state")).await;
        let d = device();
        let source = Arc::new(HeldTrust {
            entered: Default::default(),
            release: Default::default(),
            writes: std::sync::atomic::AtomicUsize::new(0),
        });
        *native.0.matrix_source.lock().unwrap() = Some(source.clone());
        let session = Arc::new(HeldTransport {
            entered: Default::default(),
            release: Default::default(),
            retired: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        });
        // This test holds the original signing callback, not actual crypto/HTTP.
        session.release.notify_one();
        let expected = session.identity();
        native.0.crypto.slots.lock().unwrap().insert(
            "agent-a".into(),
            Slot {
                authority: authority(&native, &d),
                pin: DeviceIdentity::from(&d),
                generation: 1,
                phase: "ready",
                error: None,
                session: Some(session),
                init: None,
                closing: None,
                verification: None,
                owner_verified: false,
                trust_error: None,
            },
        );
        {
            let mut slots = native.0.crypto.slots.lock().unwrap();
            let slot = slots.get_mut("agent-a").unwrap();
            slot.owner_verified = true;
            slot.phase = "unavailable";
            assert_eq!(view(Some(slot))["ownerVerified"], false);
            slot.phase = "ready";
            slot.owner_verified = false;
        }
        for field in [
            "agentId",
            "agentGeneration",
            "matrixUserId",
            "matrixDeviceId",
            "ed25519",
            "curve25519",
            "masterKey",
        ] {
            let mut changed = serde_json::to_value(&expected).unwrap();
            changed[field] = if field == "agentGeneration" {
                json!(2)
            } else {
                json!("changed")
            };
            let changed = serde_json::from_value(changed).unwrap();
            assert_eq!(
                verify(&native, &changed, &DeviceIdentity::from(&d))
                    .await
                    .err()
                    .unwrap()
                    .code,
                "agent_crypto_identity_changed"
            );
        }
        assert_eq!(source.writes.load(Ordering::Acquire), 0);
        let mut forged = serde_json::to_value(&expected).unwrap();
        forged["ownerVerified"] = json!(true);
        assert!(serde_json::from_value::<AgentCryptoIdentity>(forged).is_err());
        let first = verify(&native, &expected, &DeviceIdentity::from(&d))
            .await
            .unwrap();
        assert_eq!(first["crypto"]["verificationPending"], true);
        source.entered.notified().await;
        assert_eq!(
            verify(&native, &expected, &DeviceIdentity::from(&d))
                .await
                .unwrap()["crypto"]["verificationPending"],
            true
        );
        assert_eq!(source.writes.load(Ordering::Acquire), 1);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), stop(&native, "agent-a"))
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(10), native.0.crypto.shutdown())
                .await
                .is_err()
        );
        source.release.notify_one();
        native.0.crypto.shutdown().await.unwrap();
        {
            let slots = native.0.crypto.slots.lock().unwrap();
            assert_eq!(view(slots.get("agent-a"))["ownerVerified"], false);
            assert_eq!(view(slots.get("agent-a"))["phase"], "stopped");
        }
        native.shutdown_checked().await.unwrap();
    }
    #[tokio::test]
    async fn agent_crypto_canceled_receive_observer_cannot_discard_original_shutdown_join() {
        let root = tempfile::tempdir().unwrap();
        let native = owner(&root.path().join("state")).await;
        let d = device();
        let session = Arc::new(HeldTransport {
            entered: Default::default(),
            release: Default::default(),
            retired: AtomicBool::new(false),
            closed: AtomicBool::new(true),
        });
        native.0.crypto.slots.lock().unwrap().insert(
            "agent-a".into(),
            Slot {
                authority: authority(&native, &d),
                pin: DeviceIdentity::from(&d),
                generation: 1,
                phase: "ready",
                error: None,
                session: Some(session),
                init: None,
                closing: None,
                verification: None,
                owner_verified: false,
                trust_error: None,
            },
        );
        let registry = &native.0.crypto;
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let completed = Arc::new(AtomicBool::new(false));
        let done = completed.clone();
        let wait = release.clone();
        let signal = entered.clone();
        let receiver = registry
            .start_receive("agent-a", async move {
                signal.notify_one();
                wait.notified().await;
                done.store(true, Ordering::Release);
                Ok(())
            })
            .unwrap();
        entered.notified().await;
        drop(receiver);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), stop(&native, "agent-a"))
                .await
                .is_err()
        );
        assert_eq!(registry.slots.lock().unwrap()["agent-a"].phase, "stopping");
        assert!(
            tokio::time::timeout(Duration::from_millis(10), registry.shutdown())
                .await
                .is_err()
        );
        assert!(!completed.load(Ordering::Acquire));
        assert!(registry.start_receive("agent-a", async { Ok(()) }).is_err());
        release.notify_one();
        registry.shutdown().await.unwrap();
        assert!(completed.load(Ordering::Acquire));
        assert_eq!(registry.receives.lock().unwrap().len(), 1);
        assert_eq!(
            registry.receives.lock().unwrap()[0].1.result(),
            Some(Ok(()))
        );
        assert_eq!(registry.slots.lock().unwrap()["agent-a"].phase, "stopped");
        native.shutdown_checked().await.unwrap();
    }
    #[tokio::test]
    async fn agent_crypto_receive_rejects_malformed_headers_before_authority_or_crypto_io() {
        let root = tempfile::tempdir().unwrap();
        let native = owner(&root.path().join("state")).await;
        let event = json!({"type":"m.room.encrypted","room_id":"!room:example.test","sender":"@owner:example.test","event_id":"$event","content":{}});
        for (field, value) in [
            ("type", json!("m.room.message")),
            ("room_id", json!("!other:example.test")),
            ("event_id", json!("invalid")),
        ] {
            let mut wrong = event.clone();
            wrong[field] = value;
            assert_eq!(
                native
                    .decrypt_agent_owner_event("agent-a", 1, "!room:example.test", wrong)
                    .await
                    .err()
                    .unwrap()
                    .code,
                "agent_crypto_invalid_event"
            );
        }
        assert!(native.0.crypto.receives.lock().unwrap().is_empty());
        native.shutdown_checked().await.unwrap();
    }
}
