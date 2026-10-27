//! Explicit isolated Synapse qualification of the normal native identity lifecycle.
#[path = "native_owner_remote_approvals_qualification/source.rs"]
mod source;
use futures::FutureExt;
use hagency::native_owner::{
    AgentCryptoAction, AgentCryptoContext, Command, MatrixAccessToken,
    MatrixTokenSource, NativeError, NativeOwner,
};
use matrix_sdk::Client;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
struct ManagedSource {
    inner: source::Source,
    prepares: AtomicUsize,
    hold: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl MatrixTokenSource for ManagedSource {
    fn access_token(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = std::result::Result<MatrixAccessToken, NativeError>>
                + Send
                + '_,
        >,
    > {
        self.inner.access_token()
    }
    fn supports_agent_crypto(&self) -> bool {
        true
    }
    fn prepare_agent_crypto(&self, context: AgentCryptoContext) -> hagency::native_owner::AgentCryptoPreparation<'_> {
        Box::pin(async move {
            self.inner.check()?;
            self.prepares.fetch_add(1, Ordering::AcqRel);
            let session = hagency_app::hagency::agent_crypto::prepare_managed(context).await?;
            // Deliberately hold the ACTUAL prepared SDK handle before returning it
            // to NativeOwner. This is a callback hold, not a held HTTP response.
            if self.hold.load(Ordering::Acquire) {
                self.entered.notify_one();
                self.release.notified().await;
            }
            Ok(session)
        })
    }
}
fn private_dir(p: &Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(p)?;
    Ok(())
}
async fn create(native: &NativeOwner, name: &str) -> Result<String> {
    let response=native.execute(Command::CreateAgent {input:json!({"displayName":name,"visibility":"private","idempotencyKey":format!("{name}-{}",uuid::Uuid::new_v4())})}).await?;
    assert_eq!(response["crypto"]["phase"], "initializing");
    Ok(response["creation"]["agent"]["id"]
        .as_str()
        .ok_or("created Agent")?
        .into())
}
async fn crypto(native: &NativeOwner, action: AgentCryptoAction) -> Result<Value> {
    Ok(native.execute(Command::AgentCrypto { action }).await?)
}
async fn ready(native: &NativeOwner, agent: &str) -> Result<Value> {
    let until = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let value = crypto(
            native,
            AgentCryptoAction::Status {
                agent: agent.into(),
            },
        )
        .await?;
        match value["crypto"]["phase"].as_str() {
            Some("ready") => {
                assert_eq!(value["crypto"]["ownerVerified"], false);
                return Ok(value["crypto"]["identity"].clone());
            }
            Some("initializing") => {}
            _ => {
                return Err(
                    format!("identity did not initialize: {}", value["crypto"]["error"]).into(),
                );
            }
        }
        if tokio::time::Instant::now() >= until {
            return Err("original initialization exceeded observation deadline".into());
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}
async fn run() -> Result<()> {
    if std::env::var("HAGENCY_AGENT_CRYPTO_QUALIFICATION").as_deref() != Ok("1") {
        return Err("explicit isolated qualification required".into());
    }
    let bootstrap_path =
        PathBuf::from(std::env::var("HAGENCY_SYNAPSE_BOOTSTRAP")?).canonicalize()?;
    let bootstrap: Value = serde_json::from_slice(&std::fs::read(&bootstrap_path)?)?;
    let info: Value =
        serde_json::from_slice(&std::fs::read(std::env::var("HAGENCY_FILE_FIXTURE_INFO")?)?)?;
    let origin = info["origin"].as_str().ok_or("origin")?;
    let url: url::Url = origin.parse()?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || bootstrap["owner"]["user_id"] != "@alice:test"
        || info["realHomeserver"] != true
    {
        return Err("isolated actual Synapse required".into());
    }
    let root = PathBuf::from(std::env::var("HAGENCY_AGENT_CRYPTO_ROOT")?);
    if root.parent().ok_or("parent")?.canonicalize()?
        != bootstrap_path.parent().ok_or("parent")?.canonicalize()?
    {
        return Err("fresh root must belong to isolated fixture".into());
    }
    private_dir(&root)?;
    private_dir(&root.join("owner-sdk"))?;
    let http = matrix_sdk::reqwest::Client::builder()
        .no_proxy()
        .redirect(matrix_sdk::reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()?;
    let owner_sdk = Client::builder()
        .homeserver_url(origin)
        .http_client(http.clone())
        .sqlite_store(root.join("owner-sdk"), None)
        .build()
        .await?;
    owner_sdk
        .matrix_auth()
        .restore_session(
            serde_json::from_value(bootstrap["owner"].clone())?,
            Default::default(),
        )
        .await?;
    let source = Arc::new(ManagedSource {
        inner: source::Source::new(owner_sdk.clone(), origin)?,
        prepares: AtomicUsize::new(0),
        hold: AtomicBool::new(false),
        entered: Default::default(),
        release: Default::default(),
    });
    let mut held = Some(
        NativeOwner::open_with_matrix(
            &root.join("native-owner"),
            origin,
            "@alice:test",
            source.clone(),
        )
        .await?,
    );
    let outcome=std::panic::AssertUnwindSafe(async {
        let native=held.as_ref().ok_or("native owner")?;
        eprintln!("qualification: normal Agent creation initializes two actual SDK identities");
        let first=create(native,"managed-first").await?;let second=create(native,"managed-second").await?;
        let a=ready(native,&first).await?;let b=ready(native,&second).await?;
        for key in ["matrixUserId","matrixDeviceId","ed25519","curve25519"] {assert_ne!(a[key],b[key]);}
        let own=owner_sdk.encryption().get_own_device().await?.ok_or("owner keys")?;
        for p in [&a,&b] {assert_ne!(p["ed25519"],own.ed25519_key().ok_or("owner Ed25519")?.to_base64());}
        drop(own);
        let device=native.authorized_execution_device().await?;let old_token=device.bearer()?.to_owned();
        eprintln!("qualification: retain original sessions through the 30-second authority deadline");
        tokio::time::sleep(Duration::from_secs(35)).await;
        assert_eq!(ready(native,&first).await?,a);assert_eq!(ready(native,&second).await?,b);
        assert_eq!(source.prepares.load(Ordering::Acquire),2);
        assert_eq!(native.authorized_execution_device().await?.generation(),device.generation());
        let actual:Value=serde_json::from_slice(&http.post(format!("{}_matrix/client/v3/keys/query",origin)).bearer_auth(owner_sdk.access_token().ok_or("owner token")?).header("content-type", "application/json").body(serde_json::to_vec(&json!({"device_keys":{a["matrixUserId"].as_str().unwrap():[],b["matrixUserId"].as_str().unwrap():[]}}))?).send().await?.error_for_status()?.bytes().await?)?;
        for p in [&a,&b] {
            let user=p["matrixUserId"].as_str().unwrap();let id=p["matrixDeviceId"].as_str().unwrap();
            assert_eq!(actual["device_keys"][user][id]["keys"][format!("ed25519:{id}")],p["ed25519"]);
            assert_eq!(actual["device_keys"][user][id]["keys"][format!("curve25519:{id}")],p["curve25519"]);
        }
        let direct=http.get(url::Url::parse(bootstrap["matrixOrigin"].as_str().ok_or("Matrix origin")?)?.join("/_matrix/client/v3/account/whoami")?).bearer_auth(&old_token).send().await?;
        assert_eq!(direct.status().as_u16(),401);
        eprintln!("qualification: real Server revocation fences original SDK authority");
        let grant:Value=serde_json::from_slice(&http.post(format!("{}hagency/api/v1/sessions/pasion",origin)).header("content-type", "application/json").body(serde_json::to_vec(&json!({"accessToken":owner_sdk.access_token().ok_or("owner token")?}))?).send().await?.error_for_status()?.bytes().await?)?;
        http.delete(format!("{}hagency/api/v1/devices/{}",origin,device.device_id())).bearer_auth(grant["token"].as_str().ok_or("owner session")?).send().await?.error_for_status()?;
        assert_eq!(http.get(format!("{}hagency/agent-crypto/{first}/1/_matrix/client/v3/account/whoami",origin)).bearer_auth(&old_token).send().await?.status().as_u16(),401);
        if let Ok(value)=crypto(native,AgentCryptoAction::Status{agent:first.clone()}).await {assert_ne!(value["crypto"]["phase"],"ready");}
        native.shutdown_checked().await?;drop(held.take());
        eprintln!("qualification: reopen same installation, reuse private material and SDK keys");
        held=Some(NativeOwner::open_with_matrix(&root.join("native-owner"),origin,"@alice:test",source.clone()).await?);
        let restarted=held.as_ref().ok_or("restarted")?;
        let current=restarted.authorized_execution_device().await?;
        assert_eq!(current.device_id(),device.device_id());assert!(current.generation()>device.generation());
        for agent in [&first,&second] {crypto(restarted,AgentCryptoAction::Prepare{agent:agent.clone()}).await?;}
        assert_eq!(ready(restarted,&first).await?,a);assert_eq!(ready(restarted,&second).await?,b);
        eprintln!("qualification: canceled stop and shutdown retain the actual prepared third SDK");
        source.hold.store(true,Ordering::Release);let third=create(restarted,"managed-held").await?;
        tokio::time::timeout(Duration::from_secs(60),source.entered.notified()).await?;
        assert!(tokio::time::timeout(Duration::from_millis(30),crypto(restarted,AgentCryptoAction::Stop{agent:third.clone()})).await.is_err());
        assert!(tokio::time::timeout(Duration::from_millis(30),restarted.shutdown_checked()).await.is_err());
        source.release.notify_one();restarted.shutdown_checked().await?;
        std::fs::write(root.join("result.json"),serde_json::to_vec_pretty(&json!({"first":a,"second":b,"heldAgentId":third,"executionDeviceId":device.device_id(),"normalCreatePrepares":true,"sameSessionAuthorityRenewalAfter35Seconds":true,"actualHomeserverPublicKeys":true,"ownerKeysIndependent":true,"nativeRestartRetainsKeys":true,"serverDeviceRevocationRefused":true,"directHomeserverBearerRefused":true,"canceledStopAndShutdownJoinOriginalPreparedCallback":true,"callbackHoldIsNotHttpHold":true,"ownerVerified":false,"modelCalls":0}))?)?;
        Ok::<_,Box<dyn std::error::Error>>(())
    }).catch_unwind().await;
    // Release the owned qualification gate on every failure before joined shutdown.
    source.hold.store(false, Ordering::Release);
    source.release.notify_one();
    let shutdown = if let Some(owner) = &held {
        owner.shutdown_checked().await
    } else {
        Ok(())
    };
    source.inner.active.store(false, Ordering::Release);
    shutdown?;
    outcome.map_err(|_| "qualification assertion failed")?
}
fn main() -> Result<()> {
    if let Some(result) = hagency::native_owner::run_guardian_if_requested() {
        result?;
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}
