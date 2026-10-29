//! Independent Agent SDK identity. No owner keys, AS credential, Room send or tool authority.
use ::hagency::{
    console::AuthorizedDevice,
    native_owner::{
        NativeOwner, NativeError, AgentCryptoAuthority, AgentCryptoContext, AgentCryptoTransport,
        AgentCryptoIdentity,
    },
};
use matrix_sdk::{
    Client, SessionMeta,
    authentication::matrix::MatrixSession,
    config::RequestConfig,
    ruma::{UserId, api::MatrixVersion},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

const MAX: usize = 131_072;
fn error(code: &str) -> NativeError {
    NativeError {
        status: 409,
        code: code.into(),
    }
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    agent_id: String,
    agent_generation: i64,
    execution_device_id: String,
    device_generation: u64,
    matrix_user_id: String,
    matrix_device_id: String,
    homeserver: String,
    server_versions: Vec<String>,
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Binding {
    origin: String,
    issuer: String,
    subject: String,
    owner: String,
    manifest: Manifest,
}
pub use ::hagency::native_owner::AgentCryptoIdentity as PublicIdentity;
struct Retirement(std::sync::Arc<matrix_sdk::HttpRequestFence>);
impl Drop for Retirement {
    fn drop(&mut self) {
        self.0.retire();
    }
}
struct Resources {
    client: Client,
    _store_lock: std::sync::Arc<std::sync::OnceLock<File>>,
}
/// A host-owned SDK session. Neither SDK Client nor bearer can escape.
pub struct AgentCryptoSession {
    authority: AgentCryptoAuthority,
    proof: std::sync::Arc<std::sync::Mutex<AuthorizedDevice>>,
    resources: tokio::sync::Mutex<Option<Resources>>,
    binding: Binding,
    retirement: Retirement,
    owner_receive: bool,
}
async fn bounded(mut response: matrix_sdk::reqwest::Response) -> Result<Value, NativeError> {
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| error("agent_crypto_outcome_unknown"))?
    {
        if bytes.len() + chunk.len() > MAX {
            return Err(error("agent_crypto_response_too_large"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| error("invalid_agent_crypto_response"))?;
    if !status.is_success() {
        return Err(NativeError {
            status: status.as_u16(),
            code: "agent_crypto_request_refused".into(),
        });
    }
    Ok(value)
}
fn check_manifest(
    m: &Manifest,
    d: &AuthorizedDevice,
    agent: &str,
    generation: i64,
    receive: bool,
) -> Result<(), NativeError> {
    let expected = format!(
        "{}hagency/agent-crypto/{agent}/{generation}{}",
        d.origin(),
        if receive { "/receive" } else { "" }
    );
    let user =
        UserId::parse(&m.matrix_user_id).map_err(|_| error("agent_crypto_identity_mismatch"))?;
    let owner =
        UserId::parse(d.owner_mxid()).map_err(|_| error("agent_crypto_identity_mismatch"))?;
    if m.agent_id != agent
        || m.agent_generation != generation
        || m.execution_device_id != d.device_id()
        || m.device_generation != d.generation()
        || m.homeserver != expected
        || user.server_name() != owner.server_name()
        || !user.localpart().starts_with("_hagency_")
        || user == owner
        || m.matrix_device_id.len() != 72
        || !m.matrix_device_id.starts_with("HAGENCY_")
        || !m.matrix_device_id[8..]
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(error("agent_crypto_identity_mismatch"));
    }
    Ok(())
}
fn private_dir(path: &Path) -> Result<(), NativeError> {
    let m = std::fs::symlink_metadata(path).map_err(|_| error("agent_crypto_store_unavailable"))?;
    if !m.is_dir() || m.file_type().is_symlink() {
        return Err(error("agent_crypto_store_unavailable"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if m.permissions().mode() & 0o077 != 0 {
            return Err(error("agent_crypto_store_not_private"));
        }
    }
    Ok(())
}
fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options
}
fn store(root: &Path, binding: &Binding) -> Result<(PathBuf, File), NativeError> {
    private_dir(root)?;
    let root = root
        .canonicalize()
        .map_err(|_| error("agent_crypto_store_unavailable"))?;
    // Advertised protocol versions may change on a homeserver upgrade; they are
    // not a new crypto identity and must not select a new key directory.
    let stable = json!({"origin":binding.origin,"issuer":binding.issuer,"subject":binding.subject,"owner":binding.owner,"agent":binding.manifest.agent_id,"executionDevice":binding.manifest.execution_device_id,"user":binding.manifest.matrix_user_id,"matrixDevice":binding.manifest.matrix_device_id});
    let bytes = serde_json::to_vec(&stable).map_err(|_| error("agent_crypto_store_unavailable"))?;
    let path = root.join(format!("agent-{:x}", Sha256::digest(&bytes)));
    if !path
        .try_exists()
        .map_err(|_| error("agent_crypto_store_unavailable"))?
    {
        let builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        builder
            .create(&path)
            .map_err(|_| error("agent_crypto_store_unavailable"))?;
    }
    private_dir(&path)?;
    let lock = options()
        .create(true)
        .truncate(false)
        .open(path.join("host.lock"))
        .map_err(|_| error("agent_crypto_store_unavailable"))?;
    lock.try_lock()
        .map_err(|_| error("agent_crypto_store_in_use"))?;
    let marker = path.join("binding.json");
    let crypto = path.join("matrix-sdk-crypto.sqlite3");
    if marker
        .try_exists()
        .map_err(|_| error("agent_crypto_store_unavailable"))?
    {
        let mut file = options()
            .open(&marker)
            .map_err(|_| error("agent_crypto_store_unavailable"))?;
        if file
            .metadata()
            .map_err(|_| error("agent_crypto_store_unavailable"))?
            .len()
            > 8192
        {
            return Err(error("agent_crypto_store_scope_changed"));
        }
        let mut old = Vec::new();
        file.read_to_end(&mut old)
            .map_err(|_| error("agent_crypto_store_unavailable"))?;
        if serde_json::from_slice::<Value>(&old).ok().as_ref() != Some(&stable) {
            return Err(error("agent_crypto_store_scope_changed"));
        }
        let m = std::fs::symlink_metadata(&crypto)
            .map_err(|_| error("agent_crypto_original_keys_missing"))?;
        if !m.is_file() || m.file_type().is_symlink() {
            return Err(error("agent_crypto_original_keys_missing"));
        }
    } else {
        if crypto
            .symlink_metadata()
            .map(|_| true)
            .or_else(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Ok(false)
                } else {
                    Err(e)
                }
            })
            .map_err(|_| error("agent_crypto_store_unavailable"))?
        {
            return Err(error("agent_crypto_store_scope_changed"));
        }
        let mut file = options()
            .create_new(true)
            .open(marker)
            .map_err(|_| error("agent_crypto_store_unavailable"))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| error("agent_crypto_store_unavailable"))?;
    }
    Ok((path, lock))
}
impl Drop for AgentCryptoSession {
    fn drop(&mut self) {
        self.retire();
    }
}
impl AgentCryptoSession {
    pub async fn open(
        owner: &NativeOwner,
        agent: &str,
        generation: i64,
        root: &Path,
        passphrase: &str,
    ) -> Result<Self, NativeError> {
        Self::open_authority(
            owner.agent_crypto_authority().await?,
            agent,
            generation,
            root,
            passphrase,
            false,
        )
        .await
    }
    pub async fn open_receiver(
        owner: &NativeOwner,
        agent: &str,
        generation: i64,
        root: &Path,
        passphrase: &str,
    ) -> Result<Self, NativeError> {
        Self::open_authority(
            owner.agent_crypto_authority().await?,
            agent,
            generation,
            root,
            passphrase,
            true,
        )
        .await
    }
    async fn open_authority(
        authority: AgentCryptoAuthority,
        agent: &str,
        generation: i64,
        root: &Path,
        passphrase: &str,
        receive: bool,
    ) -> Result<Self, NativeError> {
        if generation < 1
            || agent.is_empty()
            || agent.len() > 128
            || !agent
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || passphrase.len() < 32
        {
            return Err(error("invalid_agent_crypto_scope"));
        }
        let device = authority.device().await?;
        let proof = std::sync::Arc::new(std::sync::Mutex::new(device.clone()));
        let live = proof.clone();
        let store_custody = std::sync::Arc::new(std::sync::OnceLock::new());
        let retained_store = store_custody.clone();
        let auth = authority.clone();
        let retirement = Retirement(std::sync::Arc::new(matrix_sdk::HttpRequestFence::new(
            move || {
                // SDK HTTP permits retain their fence through the actual body.
                // Keep the store lease with that fence even if a host is dropped.
                let _original_store = retained_store.clone();
                !auth.is_retired() && live.lock().is_ok_and(|d| d.bearer().is_ok())
            },
        )));
        let http = matrix_sdk::reqwest::Client::builder()
            .no_proxy()
            .redirect(matrix_sdk::reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| error("agent_crypto_transport_unavailable"))?;
        let manifest = {
            let _permit = retirement
                .0
                .begin_request()
                .map_err(|_| error("agent_crypto_retired"))?;
            let response = http
                .post(format!(
                    "{}hagency/agent-crypto/{agent}/{generation}/{}",
                    device.origin(),
                    if receive { "receive" } else { "provision" }
                ))
                .bearer_auth(
                    device
                        .bearer()
                        .map_err(|_| error("owner_authorization_required"))?,
                )
                .send()
                .await
                .map_err(|_| error("agent_crypto_outcome_unknown"))?;
            serde_json::from_value::<Manifest>(bounded(response).await?)
                .map_err(|_| error("invalid_agent_crypto_response"))?
        };
        check_manifest(&manifest, &device, agent, generation, receive)?;
        authority.device().await?;
        let binding = Binding {
            origin: device.origin().into(),
            issuer: device.issuer().into(),
            subject: device.subject().into(),
            owner: device.owner_mxid().into(),
            manifest,
        };
        let (path, store_lock) = store(root, &binding)?;
        store_custody
            .set(store_lock)
            .map_err(|_| error("agent_crypto_store_in_use"))?;
        let versions = binding
            .manifest
            .server_versions
            .iter()
            .filter_map(|s| s.parse::<MatrixVersion>().ok())
            .collect::<Vec<_>>();
        if versions.is_empty() {
            return Err(error("invalid_agent_crypto_response"));
        }
        let client = Client::builder()
            .homeserver_url(&binding.manifest.homeserver)
            .http_client(http)
            .server_versions(versions)
            .request_fence(retirement.0.clone())
            .request_config(
                RequestConfig::new()
                    .disable_retry()
                    .force_auth()
                    .timeout(Duration::from_secs(15)),
            )
            .sqlite_store(&path, Some(passphrase))
            .build()
            .await
            .map_err(|_| error("agent_crypto_sdk_store_unavailable"))?;
        client
            .matrix_auth()
            .restore_session(
                MatrixSession {
                    meta: SessionMeta {
                        user_id: UserId::parse(&binding.manifest.matrix_user_id)
                            .map_err(|_| error("agent_crypto_identity_mismatch"))?,
                        device_id: binding.manifest.matrix_device_id.clone().into(),
                    },
                    tokens: matrix_sdk::authentication::SessionTokens {
                        access_token: device
                            .bearer()
                            .map_err(|_| error("owner_authorization_required"))?
                            .into(),
                        refresh_token: None,
                    },
                },
                Default::default(),
            )
            .await
            .map_err(|_| error("agent_crypto_sdk_restore_failed"))?;
        client
            .encryption()
            .wait_for_e2ee_initialization_tasks()
            .await;
        let session = Self {
            authority,
            proof,
            resources: tokio::sync::Mutex::new(Some(Resources {
                client,
                _store_lock: store_custody,
            })),
            binding,
            retirement,
            owner_receive: receive,
        };
        session.whoami().await?;
        Ok(session)
    }
    async fn current(&self, client: &Client) -> Result<AuthorizedDevice, NativeError> {
        let current = self.authority.device().await?;
        if client.access_token().as_deref()
            != Some(
                current
                    .bearer()
                    .map_err(|_| error("owner_authorization_required"))?,
            )
        {
            return Err(error("agent_crypto_execution_device_changed"));
        }
        *self
            .proof
            .lock()
            .map_err(|_| error("agent_crypto_retired"))? = current.clone();
        Ok(current)
    }
    async fn who(&self, client: &Client) -> Result<(), NativeError> {
        self.current(client).await?;
        let who = client
            .whoami()
            .await
            .map_err(|_| error("agent_crypto_identity_unavailable"))?;
        self.current(client).await?;
        if who.user_id.as_str() != self.binding.manifest.matrix_user_id
            || who.device_id.as_deref() != client.device_id()
            || who.is_guest
        {
            return Err(error("agent_crypto_identity_mismatch"));
        }
        Ok(())
    }
    pub async fn whoami(&self) -> Result<(), NativeError> {
        let resources = self.resources.lock().await;
        let resources = resources
            .as_ref()
            .ok_or_else(|| error("agent_crypto_retired"))?;
        self.who(&resources.client).await
    }
    async fn identity(&self, client: &Client) -> Result<PublicIdentity, NativeError> {
        self.current(client).await?;
        let device = client
            .encryption()
            .get_own_device()
            .await
            .map_err(|_| error("agent_crypto_keys_unavailable"))?
            .ok_or_else(|| error("agent_crypto_keys_unavailable"))?;
        let identity = PublicIdentity {
            agent_id: self.binding.manifest.agent_id.clone(),
            agent_generation: self.binding.manifest.agent_generation,
            matrix_user_id: self.binding.manifest.matrix_user_id.clone(),
            matrix_device_id: self.binding.manifest.matrix_device_id.clone(),
            ed25519: device
                .ed25519_key()
                .ok_or_else(|| error("agent_crypto_keys_unavailable"))?
                .to_base64(),
            master_key: client
                .encryption()
                .get_user_identity(
                    client
                        .user_id()
                        .ok_or_else(|| error("agent_crypto_identity_mismatch"))?,
                )
                .await
                .map_err(|_| error("agent_crypto_keys_unavailable"))?
                .and_then(|i| i.master_key().get_first_key())
                .ok_or_else(|| error("agent_crypto_keys_unavailable"))?
                .to_base64(),
            curve25519: device
                .curve25519_key()
                .ok_or_else(|| error("agent_crypto_keys_unavailable"))?
                .to_base64(),
        };
        let device = self.current(client).await?;
        let uploaded = {
            let _permit = self
                .retirement
                .0
                .begin_request()
                .map_err(|_| error("agent_crypto_retired"))?;
            let response=client.http_client().post(format!("{}/_matrix/client/v3/keys/query",self.binding.manifest.homeserver)).bearer_auth(device.bearer().map_err(|_|error("owner_authorization_required"))?).header("content-type","application/json").body(serde_json::to_vec(&json!({"device_keys":{&identity.matrix_user_id:[&identity.matrix_device_id]}})).map_err(|_|error("invalid_agent_crypto_scope"))?).send().await.map_err(|_|error("agent_crypto_outcome_unknown"))?;
            bounded(response).await?
        };
        self.current(client).await?;
        let keys =
            &uploaded["device_keys"][&identity.matrix_user_id][&identity.matrix_device_id]["keys"];
        if keys[format!("ed25519:{}", identity.matrix_device_id)] != identity.ed25519
            || keys[format!("curve25519:{}", identity.matrix_device_id)] != identity.curve25519
            || uploaded["master_keys"][&identity.matrix_user_id]["keys"]
                [format!("ed25519:{}", identity.master_key)]
                != identity.master_key
        {
            return Err(error("agent_crypto_identity_mismatch"));
        }
        Ok(identity)
    }
    pub async fn bootstrap(&self) -> Result<PublicIdentity, NativeError> {
        let resources = self.resources.lock().await;
        let resources = resources
            .as_ref()
            .ok_or_else(|| error("agent_crypto_retired"))?;
        let client = &resources.client;
        self.who(client).await?;
        client
            .encryption()
            .bootstrap_cross_signing(None)
            .await
            .map_err(|_| error("agent_crypto_bootstrap_failed"))?;
        self.identity(client).await
    }
    async fn refresh(&self) -> Result<PublicIdentity, NativeError> {
        let resources = self.resources.lock().await;
        let resources = resources
            .as_ref()
            .ok_or_else(|| error("agent_crypto_retired"))?;
        self.who(&resources.client).await?;
        self.identity(&resources.client).await
    }
    fn retire(&self) {
        self.retirement.0.retire();
    }
    pub async fn close(&self) -> Result<(), NativeError> {
        self.retire();
        let resources = self.resources.lock().await.take();
        if let Some(resources) = resources {
            resources
                .client
                .encryption()
                .wait_for_e2ee_initialization_tasks()
                .await;
            self.retirement
                .0
                .retire_and_drain()
                .await
                .map_err(|_| error("agent_crypto_shutdown_failed"))?;
            drop(resources);
        }
        Ok(())
    }
}
struct Managed {
    session: AgentCryptoSession,
    identity: AgentCryptoIdentity,
    receiver: Option<owner_receive::OwnerContext>,
}
impl AgentCryptoTransport for Managed {
    fn receive_status(&self) -> Value {
        self.receiver
            .as_ref()
            .map_or_else(|| json!({"phase":"unsupported"}), |r| r.status())
    }
    fn decrypt_owner_event<'a>(
        &'a self,
        room: &'a str,
        event: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, NativeError>> + Send + 'a>>
    {
        Box::pin(async move {
            self.session
                .decrypt_owner_event(
                    self.receiver
                        .as_ref()
                        .ok_or_else(|| error("agent_crypto_receive_unavailable"))?,
                    room,
                    event,
                )
                .await
        })
    }
    fn seal_reply<'a>(
        &'a self,
        room: &'a str,
        content: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, NativeError>> + Send + 'a>>
    {
        Box::pin(async move {
            self.session
                .seal_reply(
                    self.receiver
                        .as_ref()
                        .ok_or_else(|| error("agent_crypto_receive_unavailable"))?,
                    room,
                    content,
                )
                .await
        })
    }
    fn upload_encrypted_file<'a>(
        &'a self,
        bytes: Vec<u8>,
        live: std::sync::Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, NativeError>> + Send + 'a>>
    {
        Box::pin(async move {
            self.session
                .upload_encrypted_file(
                    self.receiver
                        .as_ref()
                        .ok_or_else(|| error("agent_crypto_receive_unavailable"))?,
                    bytes,
                    live,
                )
                .await
        })
    }
    fn identity(&self) -> AgentCryptoIdentity {
        self.identity.clone()
    }
    fn retire(&self) {
        self.session.retire();
    }
    fn refresh(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<AgentCryptoIdentity, NativeError>> + Send + '_>,
    > {
        Box::pin(async move {
            let actual = self.session.refresh().await?;
            if actual != self.identity {
                return Err(error("agent_crypto_identity_mismatch"));
            }
            Ok(actual)
        })
    }
    fn close(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), NativeError>> + Send + '_>>
    {
        Box::pin(self.session.close())
    }
}
pub async fn prepare_managed(
    context: AgentCryptoContext,
) -> Result<std::sync::Arc<dyn AgentCryptoTransport>, NativeError> {
    let session = AgentCryptoSession::open_authority(
        context.authority(),
        context.agent(),
        context.generation(),
        context.root(),
        context.passphrase(),
        false,
    )
    .await?;
    let identity = match session.bootstrap().await {
        Ok(identity) => identity,
        Err(e) => {
            session.close().await?;
            return Err(e);
        }
    };
    Ok(std::sync::Arc::new(Managed {
        session,
        identity,
        receiver: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn binding() -> Binding {
        Binding {
            origin: "https://example.test/".into(),
            issuer: "https://example.test/auth/".into(),
            subject: "owner-subject".into(),
            owner: "@owner:example.test".into(),
            manifest: Manifest {
                agent_id: "agt_test".into(),
                agent_generation: 1,
                execution_device_id: "device".into(),
                device_generation: 1,
                matrix_user_id: "@_hagency_agt_test:example.test".into(),
                matrix_device_id: format!("HAGENCY_{}", "a".repeat(64)),
                homeserver: "https://example.test/hagency/agent-crypto/agt_test/1".into(),
                server_versions: vec!["v1.15".into()],
            },
        }
    }
    fn root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        root
    }
    #[test]
    fn private_crypto_store_retains_exclusive_original_scope_and_keys() {
        let root = root();
        let mut binding = binding();
        let (path, lock) = store(root.path(), &binding).unwrap();
        assert_eq!(
            store(root.path(), &binding).err().unwrap().code,
            "agent_crypto_store_in_use"
        );
        drop(lock);
        assert_eq!(
            store(root.path(), &binding).err().unwrap().code,
            "agent_crypto_original_keys_missing"
        );
        std::fs::write(
            path.join("matrix-sdk-crypto.sqlite3"),
            "test store existence only",
        )
        .unwrap();
        binding.manifest.server_versions.push("v1.17".into());
        binding.manifest.agent_generation = 2;
        binding.manifest.device_generation = 3;
        binding.manifest.homeserver =
            "https://example.test/hagency/agent-crypto/agt_test/2/receive".into();
        let (same, lock) = store(root.path(), &binding).unwrap();
        assert_eq!(same, path);
        drop(lock);
        std::fs::write(path.join("binding.json"), "{}").unwrap();
        assert_eq!(
            store(root.path(), &binding).err().unwrap().code,
            "agent_crypto_store_scope_changed"
        );
    }
    #[cfg(unix)]
    #[test]
    fn crypto_store_refuses_public_directory_and_symlinked_original_keys() {
        use std::os::unix::{
            fs::{PermissionsExt, symlink},
        };
        let root = root();
        let binding = binding();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            store(root.path(), &binding).err().unwrap().code,
            "agent_crypto_store_not_private"
        );
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let (path, lock) = store(root.path(), &binding).unwrap();
        drop(lock);
        symlink(
            root.path().join("missing-original"),
            path.join("matrix-sdk-crypto.sqlite3"),
        )
        .unwrap();
        assert_eq!(
            store(root.path(), &binding).err().unwrap().code,
            "agent_crypto_original_keys_missing"
        );
    }
}

mod owner_verification;
pub use owner_verification::owner_trust;

mod owner_receive;
pub use owner_receive::prepare_managed_receiver;
