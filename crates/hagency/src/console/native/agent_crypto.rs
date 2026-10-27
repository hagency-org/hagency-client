//! Host-only Agent crypto lifecycle. Identity readiness never grants message/tool authority.
use super::*;
use crate::console::{AuthorizedDevice, device_execution::DeviceIdentity, owned_agents};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};
use zeroize::Zeroizing;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AgentCryptoAction {
    Prepare { agent: String },
    Status { agent: String },
    Stop { agent: String },
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
}
/// Weak ownership prevents a managed SDK session keeping its NativeOwner alive.
#[derive(Clone)]
pub struct AgentCryptoAuthority {
    owner: Weak<Inner>,
    pin: DeviceIdentity,
    retired: Arc<AtomicBool>,
}
impl AgentCryptoAuthority {
    pub fn retire(&self) {
        self.retired.store(true, Ordering::Release);
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
pub trait AgentCryptoTransport: Send + Sync {
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
fn material(
    native: &NativeOwner,
    d: &AuthorizedDevice,
    agent: &str,
    generation: i64,
    authority: AgentCryptoAuthority,
) -> Result<AgentCryptoContext, NativeError> {
    use std::io::Read;
    let root = native
        .0
        .host
        .console
        .0
        .server_login
        .state_directory()
        .ok_or_else(unavailable)?;
    let ledger =
        owned_agents::ledger_path(&root, d.origin(), d.issuer(), d.subject(), d.owner_mxid())
            .map_err(|_| unavailable())?;
    let profile =
        owned_agents::profile_identity(d.origin(), d.issuer(), d.subject(), d.owner_mxid())
            .map_err(|_| unavailable())?;
    let parent = ledger
        .parent()
        .ok_or_else(unavailable)?
        .join("agent-crypto");
    hagency_store::private::directory(&parent).map_err(|_| unavailable())?;
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(
        serde_json::to_vec(&(&profile, agent, d.device_id())).map_err(|_| unavailable())?,
    );
    let directory = parent.join(format!("agent_{digest:x}"));
    hagency_store::private::directory(&directory).map_err(|_| unavailable())?;
    let path = directory.join("credential.json");
    let credential = match hagency_store::private::open(&path, false) {
        Ok(file) => {
            let mut bytes = Zeroizing::new(Vec::new());
            file.take(16385)
                .read_to_end(&mut bytes)
                .map_err(|_| unavailable())?;
            if bytes.len() > 16384 {
                return Err(unavailable());
            }
            serde_json::from_slice::<Credential>(&bytes).map_err(|_| unavailable())?
        }
        Err(_)
            if std::fs::symlink_metadata(&path)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            if std::fs::read_dir(&directory)
                .map_err(|_| unavailable())?
                .next()
                .is_some()
            {
                return Err(failure(409, "agent_crypto_original_secret_missing"));
            }
            let mut random = Zeroizing::new([0u8; 48]);
            getrandom::fill(random.as_mut()).map_err(|_| unavailable())?;
            use base64::Engine;
            let credential = Credential {
                version: 1,
                profile: profile.clone(),
                agent: agent.into(),
                device: d.device_id().into(),
                passphrase: base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(random.as_slice()),
            };
            let bytes = Zeroizing::new(serde_json::to_vec(&credential).map_err(|_| unavailable())?);
            hagency_store::private::write_new(&path, &bytes).map_err(|_| unavailable())?;
            credential
        }
        Err(_) => return Err(unavailable()),
    };
    if credential.version != 1
        || credential.profile != profile
        || credential.agent != agent
        || credential.device != d.device_id()
        || credential.passphrase.len() != 64
    {
        return Err(unavailable());
    }
    let sdk = directory.join("sdk");
    hagency_store::private::directory(&sdk).map_err(|_| unavailable())?;
    let mut credential = credential;
    Ok(AgentCryptoContext {
        authority,
        agent: agent.into(),
        generation,
        root: sdk,
        passphrase: Zeroizing::new(std::mem::take(&mut credential.passphrase)),
    })
}

static STARTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
#[derive(Default)]
pub(super) struct Registry {
    closed: AtomicBool,
    slots: StdMutex<BTreeMap<String, Slot>>,
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
}
fn view(slot: Option<&Slot>) -> Value {
    slot.map_or_else(||json!({"phase":"not_initialized"}),|s|json!({"phase":s.phase,"agentGeneration":s.generation,"error":s.error,"identity":s.session.as_ref().map(|h|h.identity()),"ownerVerified":false}))
}
impl Registry {
    pub(super) fn retire(&self) {
        self.closed.store(true, Ordering::Release);
        if let Ok(mut slots) = self.slots.lock() {
            for s in slots.values_mut() {
                s.authority.retire();
                s.phase = "stopped";
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
            .flat_map(|s| [s.init.clone(), s.closing.clone()].into_iter().flatten())
            .collect::<Vec<_>>();
        let mut result = Ok(());
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
            let init = slot.init.clone();
            let session = slot.session.clone();
            let weak = Arc::downgrade(&native.0);
            let id = agent.to_owned();
            let update = crate::update_guard::PROCESS
                .continue_task()
                .ok_or_else(|| failure(409, "update_in_progress"))?;
            slot.closing = Some(crate::shutdown::Task::start(async move {
                let _update = update;
                let mut result = if let Some(init) = init {
                    init.wait().await
                } else {
                    Ok(())
                };
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
pub(super) async fn execute(
    native: &NativeOwner,
    action: AgentCryptoAction,
) -> Result<Value, NativeError> {
    let agent = match &action {
        AgentCryptoAction::Prepare { agent }
        | AgentCryptoAction::Status { agent }
        | AgentCryptoAction::Stop { agent } => agent,
    };
    key(agent)?;
    let d = native.authorized_execution_device().await?;
    let pin = DeviceIdentity::from(&d);
    let registry = &native.0.crypto;
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
    };
    let context = material(native, &d, agent, generation, authority.clone())?;
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
        let result = source.prepare_agent_crypto(context).await;
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
            && identity.curve25519.len() == 43;
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
        },
    );
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
        NativeOwner::open_inner(root, "http://127.0.0.1:9", "@owner:example.test", false)
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
        }
    }
    #[tokio::test]
    async fn agent_crypto_private_material_is_scoped_and_retains_original_keys_across_epochs() {
        let root = tempfile::tempdir().unwrap();
        let owner = owner(&root.path().join("state")).await;
        let d = device();
        let a = material(&owner, &d, "agent-a", 1, authority(&owner, &d)).unwrap();
        let again = material(&owner, &d, "agent-a", 2, authority(&owner, &d)).unwrap();
        assert_eq!(a.root, again.root);
        assert_eq!(a.passphrase, again.passphrase);
        let b = material(&owner, &d, "agent-b", 1, authority(&owner, &d)).unwrap();
        assert_ne!(a.root, b.root);
        assert_ne!(a.passphrase, b.passphrase);
        let other = crate::console::server_login::fixture_device(
            "http://127.0.0.1:9/",
            "other",
            "@other:example.test",
        )
        .0;
        let c = material(&owner, &other, "agent-a", 1, authority(&owner, &other)).unwrap();
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
        let original = material(&owner, &d, "agent-a", 1, authority(&owner, &d)).unwrap();
        let path = original.root.parent().unwrap().join("credential.json");
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            material(&owner, &d, "agent-a", 1, authority(&owner, &d))
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
        let original = material(&owner, &d, "agent-a", 1, authority(&owner, &d)).unwrap();
        let path = original.root.parent().unwrap().join("credential.json");
        let target = root.path().join("untouched");
        std::fs::write(&target, b"original").unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(material(&owner, &d, "agent-a", 1, authority(&owner, &d)).is_err());
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
}
