//! Explicit actual Synapse/native owner-to-Agent cross-signing qualification.
#[path = "native_owner_remote_approvals_qualification/source.rs"]
mod source;
use futures::FutureExt;
use hagency::native_owner::{
    AgentCryptoAction, AgentCryptoContext, AgentCryptoAuthority, AgentCryptoIdentity, Command,
    MatrixAccessToken, MatrixTokenSource, NativeError, NativeOwner,
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
struct OwnerSource {
    inner: source::Source,
    verifies: AtomicUsize,
    hold: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl MatrixTokenSource for OwnerSource {
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
    fn prepare_agent_crypto(
        &self,
        context: AgentCryptoContext,
    ) -> hagency::native_owner::AgentCryptoPreparation<'_> {
        Box::pin(async move {
            self.inner.check()?;
            hagency_app::hagency::agent_crypto::prepare_managed(context).await
        })
    }
    fn agent_crypto_owner_trust(
        &self,
        identity: AgentCryptoIdentity,
        authority: AgentCryptoAuthority,
        verify: bool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::result::Result<bool, NativeError>> + Send + '_>,
    > {
        Box::pin(async move {
            self.inner.check()?;
            if verify {
                self.verifies.fetch_add(1, Ordering::AcqRel);
            }
            let active = self.inner.active.clone();
            let result = hagency_app::hagency::agent_crypto::owner_trust(
                &self.inner.client,
                identity,
                authority,
                verify,
                Arc::new(move || active.load(Ordering::Acquire)),
            )
            .await;
            // Hold the original callback AFTER actual signature publication/reobservation.
            // This does not simulate an HTTP body still being in flight.
            if verify && self.hold.load(Ordering::Acquire) {
                self.entered.notify_one();
                self.release.notified().await;
            }
            result
        })
    }
}
async fn status(native: &NativeOwner, agent: &str) -> Result<Value> {
    crypto(
        native,
        AgentCryptoAction::Status {
            agent: agent.into(),
        },
    )
    .await
}
async fn settled(native: &NativeOwner, agent: &str) -> Result<Value> {
    let until = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let value = status(native, agent).await?;
        if value["crypto"]["phase"] == "ready" && value["crypto"]["verificationPending"] != true {
            return Ok(value);
        }
        if !matches!(
            value["crypto"]["phase"].as_str(),
            Some("initializing" | "ready")
        ) {
            return Err(format!("identity unavailable: {}", value["crypto"]["error"]).into());
        }
        if tokio::time::Instant::now() >= until {
            return Err("original operation exceeded observation deadline".into());
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
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
    let source = Arc::new(OwnerSource {
        inner: source::Source::new(owner_sdk.clone(), origin)?,
        verifies: AtomicUsize::new(0),
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
        eprintln!("qualification: create two independent Agents, reject unverified owner keys");
        let first=create(native,"trust-first").await?;let second=create(native,"trust-second").await?;
        let a=settled(native,&first).await?["crypto"]["identity"].clone();
        let b=settled(native,&second).await?["crypto"]["identity"].clone();
        let mut wrong=a.clone();wrong["masterKey"]=json!("X".repeat(43));
        assert_eq!(native.execute(Command::AgentCrypto{action:AgentCryptoAction::Verify{identity:serde_json::from_value(wrong)?}}).await.err().ok_or("changed keys accepted")?.code,"agent_crypto_identity_changed");
        assert_eq!(source.verifies.load(Ordering::Acquire),0);
        crypto(native,AgentCryptoAction::Verify{identity:serde_json::from_value(a.clone())?}).await?;
        let unsigned=settled(native,&first).await?;
        assert_eq!(unsigned["crypto"]["ownerVerified"],false);
        assert_eq!(unsigned["crypto"]["verificationError"],"agent_crypto_owner_keys_unverified");
        eprintln!("qualification: actual owner cross-signing bootstrap, exact Agent signature and fresh verification");
        if let Err(error)=owner_sdk.encryption().bootstrap_cross_signing(None).await {
            use matrix_sdk::ruma::api::client::uiaa;
            let response=error.as_uiaa_response().ok_or(error.to_string())?;
            let password=std::fs::read_to_string(bootstrap_path.parent().ok_or("parent")?.join("owner-password.txt"))?;
            let mut auth=uiaa::Password::new(uiaa::UserIdentifier::Matrix(uiaa::MatrixUserIdentifier::new("alice".into())),password);
            auth.session=response.session.clone();owner_sdk.encryption().bootstrap_cross_signing(Some(uiaa::AuthData::Password(auth))).await?;
        }
        eprintln!("qualification: a genuinely cross-signed owner device without private user-signing keys cannot sign an Agent");
        let password=std::fs::read_to_string(bootstrap_path.parent().ok_or("parent")?.join("owner-password.txt"))?;
        let login:Value=serde_json::from_slice(&http.post(format!("{}_matrix/client/v3/login",bootstrap["matrixOrigin"].as_str().ok_or("matrix origin")?)).header("content-type","application/json").body(serde_json::to_vec(&json!({"type":"m.login.password","identifier":{"type":"m.id.user","user":"alice"},"password":password,"device_id":"OWNER-NO-SIGNER"}))?).send().await?.error_for_status()?.bytes().await?)?;
        private_dir(&root.join("owner-without-signing"))?;
        let without=Client::builder().homeserver_url(origin).http_client(http.clone()).sqlite_store(root.join("owner-without-signing"),None).build().await?;
        without.matrix_auth().restore_session(serde_json::from_value(login)?,Default::default()).await?;
        without.encryption().wait_for_e2ee_initialization_tasks().await;
        without.sync_once(matrix_sdk::config::SyncSettings::default().timeout(Duration::from_millis(10))).await?;
        let owner_id=owner_sdk.user_id().ok_or("owner id")?;
        owner_sdk.encryption().request_user_identity(owner_id).await?;
        owner_sdk.encryption().get_device(owner_id,matrix_sdk::ruma::device_id!("OWNER-NO-SIGNER")).await?.ok_or("new actual owner device")?.verify().await?;
        let root_identity=without.encryption().request_user_identity(owner_id).await?.ok_or("public owner identity")?;
        assert_eq!(root_identity.master_key().get_first_key(),owner_sdk.encryption().get_user_identity(owner_id).await?.ok_or("original owner identity")?.master_key().get_first_key());
        root_identity.verify().await?;
        without.encryption().request_user_identity(owner_id).await?;
        assert!(without.encryption().get_own_device().await?.ok_or("actual no-signer device")?.is_verified_with_cross_signing());
        assert!(!without.encryption().cross_signing_status().await.ok_or("signing status")?.has_user_signing);
        let unavailable=hagency_app::hagency::agent_crypto::owner_trust(&without,serde_json::from_value(b.clone())?,native.agent_crypto_authority().await?,true,Arc::new(||true)).await.err().ok_or("missing private signer accepted")?;
        assert_eq!(unavailable.code,"agent_crypto_owner_signing_keys_unavailable");
        drop(without);
        crypto(native,AgentCryptoAction::Verify{identity:serde_json::from_value(a.clone())?}).await?;
        assert_eq!(settled(native,&first).await?["crypto"]["ownerVerified"],true);
        assert_eq!(settled(native,&second).await?["crypto"]["ownerVerified"],false);
        let peer=owner_sdk.encryption().request_user_identity(&matrix_sdk::ruma::UserId::parse(a["matrixUserId"].as_str().ok_or("user")?)?).await?.ok_or("actual identity")?;
        assert!(peer.is_verified());assert_eq!(peer.master_key().get_first_key().ok_or("master")?.to_base64(),a["masterKey"]);
        let old_device=native.authorized_execution_device().await?;
        native.shutdown_checked().await?;drop(held.take());
        eprintln!("qualification: restart native owner; actual SDK trust and Agent keys persist");
        held=Some(NativeOwner::open_with_matrix(&root.join("native-owner"),origin,"@alice:test",source.clone()).await?);
        let restarted=held.as_ref().ok_or("restarted")?;
        let current=restarted.authorized_execution_device().await?;assert_eq!(current.device_id(),old_device.device_id());assert!(current.generation()>old_device.generation());
        for agent in [&first,&second]{crypto(restarted,AgentCryptoAction::Prepare{agent:agent.clone()}).await?;}
        let restored=settled(restarted,&first).await?;assert_eq!(restored["crypto"]["identity"],a);assert_eq!(restored["crypto"]["ownerVerified"],true);
        assert_eq!(settled(restarted,&second).await?["crypto"]["identity"],b);
        eprintln!("qualification: original late verified callback cannot bypass joined retirement");
        source.hold.store(true,Ordering::Release);
        crypto(restarted,AgentCryptoAction::Verify{identity:serde_json::from_value(b.clone())?}).await?;
        tokio::time::timeout(Duration::from_secs(60),source.entered.notified()).await?;
        assert_eq!(status(restarted,&second).await?["crypto"]["verificationPending"],true);
        assert!(tokio::time::timeout(Duration::from_millis(30),crypto(restarted,AgentCryptoAction::Stop{agent:second.clone()})).await.is_err());
        assert!(tokio::time::timeout(Duration::from_millis(30),restarted.shutdown_checked()).await.is_err());
        source.release.notify_one();restarted.shutdown_checked().await?;
        std::fs::write(root.join("result.json"),serde_json::to_vec_pretty(&json!({"first":a,"second":b,"executionDeviceId":current.device_id(),"deviceGenerationBefore":old_device.generation(),"deviceGenerationAfter":current.generation(),"changedMasterRefusedBeforeSigning":true,"unverifiedOwnerKeysRefused":true,"verifiedOwnerDeviceWithoutPrivateSignerRefused":true,"actualOwnerCrossSigningAndRequery":true,"secondAgentNotImplicitlyVerified":true,"nativeRestartRetainsVerifiedIdentity":true,"canceledStopAndShutdownJoinOriginalVerifiedCallback":true,"callbackHoldIsNotHttpHold":true,"modelCalls":0}))?)?;
        Ok::<_,Box<dyn std::error::Error>>(())
    }).catch_unwind().await;
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
