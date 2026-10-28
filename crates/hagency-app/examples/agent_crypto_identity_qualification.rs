//! Explicit real Synapse/SDK/native qualification; no model invocation.
#[path = "native_owner_remote_approvals_qualification/source.rs"]
mod source;
use futures::FutureExt;
use hagency::native_owner::{NativeOwner, Command};
use hagency_app::hagency::agent_crypto::AgentCryptoSession;
use matrix_sdk::Client;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
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
    let response = native
        .execute(Command::CreateAgent {
            input: json!({"displayName":name,"visibility":"private","idempotencyKey":format!("{name}-{}",uuid::Uuid::new_v4())}),
        })
        .await?;
    Ok(response["creation"]["agent"]["id"]
        .as_str()
        .ok_or("created Agent")?
        .into())
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
    if root.parent().ok_or("root parent")?.canonicalize()?
        != bootstrap_path.parent().ok_or("bootstrap parent")?
    {
        return Err("fresh root must belong to the isolated fixture".into());
    }
    private_dir(&root)?;
    private_dir(&root.join("owner-sdk"))?;
    private_dir(&root.join("agent-stores"))?;
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
    let source = Arc::new(source::Source::new(owner_sdk.clone(), origin)?);
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
        let native=held.as_ref().ok_or("original native owner")?;
        let first=create(native,"crypto-first").await?;
        let second=create(native,"crypto-second").await?;
        let device=native.authorized_execution_device().await?;
        let token=device.bearer()?.to_owned();
        use rand::{Rng,distributions::Alphanumeric};
        let passphrase:String=rand::thread_rng().sample_iter(Alphanumeric).take(64).map(char::from).collect();
        {use std::io::Write;let mut options=std::fs::OpenOptions::new();options.write(true).create_new(true);#[cfg(unix)]{use std::os::unix::fs::OpenOptionsExt;options.mode(0o600);}let mut file=options.open(root.join("store-passphrase"))?;file.write_all(passphrase.as_bytes())?;file.sync_all()?;}
        eprintln!("qualification: bootstrap first independent Agent SDK");
        let a=AgentCryptoSession::open(native,&first,1,&root.join("agent-stores"),&passphrase).await?;
        let public_a=a.bootstrap().await?;
        eprintln!("qualification: bootstrap second independent Agent SDK");
        let b=AgentCryptoSession::open(native,&second,1,&root.join("agent-stores"),&passphrase).await?;
        let public_b=b.bootstrap().await?;
        assert_ne!(public_a.matrix_user_id,public_b.matrix_user_id);
        assert_ne!(public_a.matrix_device_id,public_b.matrix_device_id);
        assert_ne!(public_a.ed25519,public_b.ed25519);assert_ne!(public_a.curve25519,public_b.curve25519);
        let own=owner_sdk.encryption().get_own_device().await?.ok_or("actual owner keys")?;
        assert_ne!(public_a.ed25519,own.ed25519_key().ok_or("owner ed25519")?.to_base64());
        assert_ne!(public_b.ed25519,own.ed25519_key().ok_or("owner ed25519")?.to_base64());
        drop(own);drop(a);drop(b);
        eprintln!("qualification: reopen original encrypted SDK store");
        let reopened=AgentCryptoSession::open(native,&first,1,&root.join("agent-stores"),&passphrase).await?;
        assert_eq!(reopened.bootstrap().await?,public_a);
        // Independently read actual homeserver public keys using the owner SDK.
        let actual:Value=serde_json::from_slice(&http.post(format!("{}_matrix/client/v3/keys/query",origin)).bearer_auth(owner_sdk.access_token().ok_or("owner token")?).header("content-type","application/json").body(serde_json::to_vec(&json!({"device_keys":{&public_a.matrix_user_id:[],&public_b.matrix_user_id:[]}}))?).send().await?.error_for_status()?.bytes().await?)?;
        for p in [&public_a,&public_b] {
            assert_eq!(actual["device_keys"][&p.matrix_user_id][&p.matrix_device_id]["keys"][format!("ed25519:{}",p.matrix_device_id)],p.ed25519);
            assert!(actual["master_keys"][&p.matrix_user_id].is_object());
            assert!(actual["self_signing_keys"][&p.matrix_user_id].is_object());
        }
        // The SDK bearer is NOT a homeserver access token.
        let direct=http.get(url::Url::parse(bootstrap["matrixOrigin"].as_str().ok_or("Matrix origin")?)?.join("/_matrix/client/v3/account/whoami")?).bearer_auth(&token).send().await?;
        assert_eq!(direct.status().as_u16(),401);
        // Revoke through the actual owner-session/device API, not a local flag.
        let grant:Value=serde_json::from_slice(&http.post(format!("{}hagency/api/v1/sessions/pasion",origin)).header("content-type","application/json").body(serde_json::to_vec(&json!({"accessToken":owner_sdk.access_token().ok_or("owner token")?}))?).send().await?.error_for_status()?.bytes().await?)?;
        http.delete(format!("{}hagency/api/v1/devices/{}",origin,device.device_id())).bearer_auth(grant["token"].as_str().ok_or("owner session")?).send().await?.error_for_status()?;
        let refused=http.get(format!("{}hagency/agent-crypto/{}/1/_matrix/client/v3/account/whoami",origin,first)).bearer_auth(&token).send().await?;
        assert_eq!(refused.status().as_u16(),401);
        assert!(reopened.whoami().await.is_err());
        drop(reopened);
        // A fresh native sign-in on the SAME stable installation rotates its
        // authorization generation, but must retain the Agent crypto keys.
        native.shutdown_checked().await?;
        // Checked shutdown retires IO; dropping the original host also releases
        // its exclusive state-directory lease before reopening that directory.
        drop(held.take());
        held=Some(NativeOwner::open_with_matrix(&root.join("native-owner"),origin,"@alice:test",source.clone()).await?);
        let restarted=held.as_ref().ok_or("restarted native owner")?;
        let restored=std::panic::AssertUnwindSafe(async {
            let current=restarted.authorized_execution_device().await?;
            assert_eq!(current.device_id(),device.device_id());
            assert!(current.generation()>device.generation());
            let session=AgentCryptoSession::open(restarted,&first,1,&root.join("agent-stores"),&passphrase).await?;
            assert_eq!(session.bootstrap().await?,public_a);
            Ok::<_,Box<dyn std::error::Error>>(())
        }).catch_unwind().await;
        let restarted_stop=restarted.shutdown_checked().await;restarted_stop?;restored.map_err(|_|"restart qualification assertion failed")??;
        std::fs::write(root.join("result.json"),serde_json::to_vec_pretty(&json!({"first":public_a,"second":public_b,"executionDeviceId":device.device_id(),"independentOwnerKeys":true,"actualHomeserverPublicKeys":true,"originalEncryptedStoreReopened":true,"nativeRestartAndCredentialRotationRetainKeys":true,"directHomeserverBearerRefused":true,"serverDeviceRevocationRefused":true,"modelCalls":0}))?)?;
        Ok::<_,Box<dyn std::error::Error>>(())
    }).catch_unwind().await;
    let shutdown = if let Some(native) = held.as_ref() {
        native.shutdown_checked().await
    } else {
        Ok(())
    };
    source.active.store(false, Ordering::Release);
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
