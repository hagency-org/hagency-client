//! Actual owner ciphertext receipt through the normal NativeOwner Agent SDK.
//! Does not admit a task, ACK a routing event, obtain a lease or run inference.
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
            let active = self.inner.active.clone();
            hagency_app::hagency::agent_crypto::prepare_managed_receiver(
                context,
                self.inner.client.clone(),
                Arc::new(move || active.load(Ordering::Acquire)),
            )
            .await
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
        eprintln!("qualification: actual owner cross-signing bootstrap");
        if let Err(error)=owner_sdk.encryption().bootstrap_cross_signing(None).await {
            use matrix_sdk::ruma::api::client::uiaa;
            let response=error.as_uiaa_response().ok_or(error.to_string())?;
            let password=std::fs::read_to_string(bootstrap_path.parent().ok_or("parent")?.join("owner-password.txt"))?;
            let mut auth=uiaa::Password::new(uiaa::UserIdentifier::Matrix(uiaa::MatrixUserIdentifier::new("alice".into())),password);
            auth.session=response.session.clone();owner_sdk.encryption().bootstrap_cross_signing(Some(uiaa::AuthData::Password(auth))).await?;
        }
        owner_sdk.sync_once(matrix_sdk::config::SyncSettings::default().timeout(Duration::from_millis(10))).await?;
        eprintln!("qualification: normal create with owner-scoped Agent receiver");
        let first=create(native,"receive-first").await?;
        let second=create(native,"receive-second").await?;
        let first_status=settled(native,&first).await?;
        let a=first_status["crypto"]["identity"].clone();
        let b=settled(native,&second).await?["crypto"]["identity"].clone();
        assert_eq!(first_status["crypto"]["receive"]["phase"],"ready","receiver: {}",first_status["crypto"]["receive"]);
        for identity in [&a,&b] {
            crypto(native,AgentCryptoAction::Verify{identity:serde_json::from_value(identity.clone())?}).await?;
        }
        assert_eq!(settled(native,&first).await?["crypto"]["ownerVerified"],true);
        assert_eq!(settled(native,&second).await?["crypto"]["ownerVerified"],true);
        eprintln!("qualification: actual owner direct Room, real Megolm send");
        let until=tokio::time::Instant::now()+Duration::from_secs(60);
        let direct=loop {
            let direct=native.execute(Command::EnsureAgentOwnerDirect{agent_id:first.clone()}).await?;
            if direct["ownerDirectRoomId"].is_string() && direct["ownerDirect"]["state"]=="active" {break direct;}
            if !matches!(direct["ownerDirect"]["phase"].as_str(),Some("waiting_identity"|"pending")) || tokio::time::Instant::now()>=until {return Err(format!("original direct Room still pending: {}",direct["ownerDirect"]).into());}
            tokio::time::sleep(Duration::from_millis(300)).await;
        };
        std::fs::write(root.join("direct-public.json"),serde_json::to_vec_pretty(&direct)?)?;
        let room_id=direct["ownerDirectRoomId"].as_str().ok_or("owner direct Room id")?;
        let room_id=matrix_sdk::ruma::RoomId::parse(room_id)?;
        let room=owner_sdk.join_room_by_id(&room_id).await?;
        owner_sdk.sync_once(matrix_sdk::config::SyncSettings::default().timeout(Duration::from_millis(10))).await?;
        room.enable_encryption().await?;
        owner_sdk.sync_once(matrix_sdk::config::SyncSettings::default().timeout(Duration::from_millis(10))).await?;
        let expected="Original owner ciphertext reaches only its native Agent SDK";
        let response=room.send(matrix_sdk::ruma::events::room::message::RoomMessageEventContent::text_plain(expected)).require_encryption().await?;
        let mut url: url::Url=origin.parse()?;
        url.path_segments_mut().map_err(|_|"origin")?.clear().extend(["_matrix","client","v3","rooms",room_id.as_str(),"event",response.response.event_id.as_str()]);
        let raw:Value=serde_json::from_slice(&http.get(url).bearer_auth(owner_sdk.access_token().ok_or("owner token")?).send().await?.error_for_status()?.bytes().await?)?;
        assert_eq!(raw["type"],"m.room.encrypted");
        assert!(raw["content"].get("body").is_none());
        let proof=native.decrypt_agent_owner_event(&first,1,room_id.as_str(),raw.clone()).await?;
        assert_eq!(proof.identity(),&serde_json::from_value::<AgentCryptoIdentity>(a.clone())?);
        assert_eq!(proof.event(),response.response.event_id.as_str());
        assert_eq!(proof.room(),room_id.as_str());
        assert_eq!(proof.sender(),"@alice:test");
        assert_eq!(proof.ciphertext_digest().len(),64);
        let plaintext=proof.into_plaintext();
        assert_eq!(plaintext["content"]["body"],expected);
        eprintln!("qualification: wrong Agent, changed room, forged sender and tampered ciphertext refuse");
        assert!(native.decrypt_agent_owner_event(&second,1,room_id.as_str(),raw.clone()).await.is_err());
        assert!(native.decrypt_agent_owner_event(&first,2,room_id.as_str(),raw.clone()).await.is_err());
        assert!(native.decrypt_agent_owner_event(&first,1,"!wrong:test",raw.clone()).await.is_err());
        let mut foreign=raw.clone();foreign["sender"]=json!("@bob:test");
        assert!(native.decrypt_agent_owner_event(&first,1,room_id.as_str(),foreign).await.is_err());
        let mut corrupt=raw.clone();corrupt["content"]["ciphertext"]=json!("corrupted");
        assert!(native.decrypt_agent_owner_event(&first,1,room_id.as_str(),corrupt).await.is_err());
        let before=native.authorized_execution_device().await?;
        native.shutdown_checked().await?;drop(held.take());
        eprintln!("qualification: native restart retains original Megolm keys and trust");
        held=Some(NativeOwner::open_with_matrix(&root.join("native-owner"),origin,"@alice:test",source.clone()).await?);
        let restarted=held.as_ref().ok_or("restarted")?;
        let after=restarted.authorized_execution_device().await?;
        assert_eq!(after.device_id(),before.device_id());assert!(after.generation()>before.generation());
        crypto(restarted,AgentCryptoAction::Prepare{agent:first.clone()}).await?;
        assert_eq!(settled(restarted,&first).await?["crypto"]["identity"],a);
        let again=restarted.decrypt_agent_owner_event(&first,1,room_id.as_str(),raw.clone()).await?;
        assert_eq!(again.into_plaintext()["content"]["body"],expected);
        restarted.shutdown_checked().await?;
        assert!(restarted.decrypt_agent_owner_event(&first,1,room_id.as_str(),raw.clone()).await.is_err());
        std::fs::write(root.join("ciphertext-public.json"),serde_json::to_vec_pretty(&raw)?)?;
        std::fs::write(root.join("result.json"),serde_json::to_vec_pretty(&json!({"first":a,"second":b,"room":room_id,"event":response.response.event_id,"actualOwnerCiphertext":true,"actualNativeOpaqueProof":true,"wrongAgentRefused":true,"wrongGenerationRefused":true,"wrongRoomRefused":true,"foreignSenderRefused":true,"tamperedCiphertextRefused":true,"nativeRestartRetainsMegolmKeys":true,"deviceGenerationBefore":before.generation(),"deviceGenerationAfter":after.generation(),"retiredHostRefused":true,"modelCalls":0,"executionGrant":false}))?)?;
        Ok::<_,Box<dyn std::error::Error>>(())
    }).catch_unwind().await;
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
