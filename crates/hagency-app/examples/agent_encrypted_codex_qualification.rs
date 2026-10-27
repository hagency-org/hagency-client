//! Actual owner encrypted attachment → native runtime → real Codex → encrypted file/reply.
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
    fn supports_agent_file_receive(&self) -> bool {
        true
    }
    fn fetch_agent_attachment(
        &self,
        attachment: hagency::native_owner::AgentAttachment,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = std::result::Result<Vec<u8>, NativeError>> + Send + '_,
        >,
    > {
        Box::pin(async move {
            self.inner.check()?;
            hagency_app::hagency::backend::fetch_agent_attachment_bytes(
                &self.inner.client,
                &self.inner.origin,
                &self.inner.owner,
                &attachment,
                || self.inner.check(),
            )
            .await
        })
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
    if std::env::var("HAGENCY_JOINT_REAL_CODEX").as_deref() != Ok("1") {
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
    if root.exists() {
        let new_task = std::env::var("HAGENCY_ENCRYPTED_NEW_TASK").as_deref() == Ok("1");
        if !new_task
            && (std::env::var("HAGENCY_ENCRYPTED_RESUME_SETUP").as_deref() != Ok("1")
                || root.join("input-event-public.json").exists())
        {
            return Err("existing original task cannot be restarted or replaced".into());
        }
    } else {
        private_dir(&root)?;
        private_dir(&root.join("owner-sdk"))?;
    }

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
    let held = Some(
        NativeOwner::open_with_matrix(
            &root.join("native-owner"),
            origin,
            "@alice:test",
            source.clone(),
        )
        .await?,
    );
    let outcome = std::panic::AssertUnwindSafe(async {
        let native = held.as_ref().ok_or("native owner")?;
        eprintln!("qualification: actual owner cross-signing bootstrap");
        if let Err(error) = owner_sdk.encryption().bootstrap_cross_signing(None).await {
            use matrix_sdk::ruma::api::client::uiaa;
            let response = error.as_uiaa_response().ok_or(error.to_string())?;
            let password = std::fs::read_to_string(
                bootstrap_path
                    .parent()
                    .ok_or("parent")?
                    .join("owner-password.txt"),
            )?;
            let mut auth = uiaa::Password::new(
                uiaa::UserIdentifier::Matrix(uiaa::MatrixUserIdentifier::new("alice".into())),
                password,
            );
            auth.session = response.session.clone();
            owner_sdk
                .encryption()
                .bootstrap_cross_signing(Some(uiaa::AuthData::Password(auth)))
                .await?;
        }
        owner_sdk
            .sync_once(
                matrix_sdk::config::SyncSettings::default().timeout(Duration::from_millis(10)),
            )
            .await?;
        eprintln!("qualification: normal create with owner-scoped Agent receiver");
        let agents = native.execute(Command::Agents).await?;
        let existing = |name: &str| {
            agents["agents"]
                .as_array()
                .and_then(|a| a.iter().find(|a| a["displayName"] == name))
                .and_then(|a| a["id"].as_str())
                .map(str::to_owned)
        };
        let first_name = if std::env::var("HAGENCY_ENCRYPTED_NEW_TASK").as_deref() == Ok("1") {
            "receive-complete-attempt2"
        } else {
            "receive-first"
        };
        let first = if let Some(id) = existing(first_name) {
            crypto(native, AgentCryptoAction::Prepare { agent: id.clone() }).await?;
            id
        } else {
            create(native, first_name).await?
        };
        let second = if let Some(id) = existing("receive-second") {
            crypto(native, AgentCryptoAction::Prepare { agent: id.clone() }).await?;
            id
        } else {
            create(native, "receive-second").await?
        };
        let first_status = settled(native, &first).await?;
        let a = first_status["crypto"]["identity"].clone();
        let b = settled(native, &second).await?["crypto"]["identity"].clone();
        assert_eq!(
            first_status["crypto"]["receive"]["phase"], "ready",
            "receiver: {}",
            first_status["crypto"]["receive"]
        );
        for identity in [&a, &b] {
            crypto(
                native,
                AgentCryptoAction::Verify {
                    identity: serde_json::from_value(identity.clone())?,
                },
            )
            .await?;
        }
        assert_eq!(
            settled(native, &first).await?["crypto"]["ownerVerified"],
            true
        );
        assert_eq!(
            settled(native, &second).await?["crypto"]["ownerVerified"],
            true
        );
        eprintln!("qualification: actual owner direct Room, real Megolm send");
        let until = tokio::time::Instant::now() + Duration::from_secs(60);
        let direct = loop {
            let direct = native
                .execute(Command::EnsureAgentOwnerDirect {
                    agent_id: first.clone(),
                })
                .await?;
            if direct["ownerDirectRoomId"].is_string() && direct["ownerDirect"]["state"] == "active"
            {
                break direct;
            }
            if !matches!(
                direct["ownerDirect"]["phase"].as_str(),
                Some("waiting_identity" | "pending")
            ) || tokio::time::Instant::now() >= until
            {
                return Err(format!(
                    "original direct Room still pending: {}",
                    direct["ownerDirect"]
                )
                .into());
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        };
        std::fs::write(
            root.join("direct-public.json"),
            serde_json::to_vec_pretty(&direct)?,
        )?;
        let room_id = direct["ownerDirectRoomId"]
            .as_str()
            .ok_or("owner direct Room id")?;
        let room_id = matrix_sdk::ruma::RoomId::parse(room_id)?;
        let room = owner_sdk.join_room_by_id(&room_id).await?;
        owner_sdk
            .sync_once(
                matrix_sdk::config::SyncSettings::default().timeout(Duration::from_millis(10)),
            )
            .await?;
        room.enable_encryption().await?;
        owner_sdk
            .sync_once(
                matrix_sdk::config::SyncSettings::default().timeout(Duration::from_millis(10)),
            )
            .await?;
        let evidence = if std::env::var("HAGENCY_ENCRYPTED_NEW_TASK").as_deref() == Ok("1") {
            let p = root.join("attempt2");
            if std::env::var("HAGENCY_ENCRYPTED_VERIFY_ONLY").as_deref() != Ok("1") {
                private_dir(&p)?;
            }
            p
        } else {
            root.clone()
        };
        if std::env::var("HAGENCY_ENCRYPTED_VERIFY_ONLY").as_deref() == Ok("1") {
            verify_restart(native, &owner_sdk, &first, &direct, &room, &evidence).await?;
        } else {
            run_encrypted_task(
                native, &owner_sdk, &source, &first, &a, &direct, &room, &evidence,
            )
            .await?;
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .catch_unwind()
    .await;
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
async fn run_encrypted_task(
    native: &NativeOwner,
    sdk: &Client,
    source: &OwnerSource,
    agent: &str,
    identity: &Value,
    direct: &Value,
    room: &matrix_sdk::Room,
    root: &Path,
) -> Result<()> {
    use hagency::native_owner::{LocalAction, ProviderAction};
    use hagency_agent_local::{
        Budget, Limit, ModelProfile, Period, Policy, RequestPolicy, ToolPolicy, codex::Programming,
    };
    let binding = direct["ownerDirect"]["bindingId"]
        .as_str()
        .ok_or("binding")?;
    let local = |action, input| Command::Local {
        agent_id: agent.into(),
        action,
        input: Some(input),
        binding_id: None,
        requester: None,
    };
    eprintln!("qualification: provider and workspace");
    let provider = native
        .execute(Command::Provider {
            action: ProviderAction::UseLocal,
        })
        .await?;
    let prepared = native
        .execute(Command::PrepareAgentWorkspace {
            agent_id: agent.into(),
        })
        .await?;
    let workspace = prepared["workspace"].as_str().ok_or("workspace")?;
    std::fs::write(root.join("workspace-public.txt"), workspace)?;
    let profile = ModelProfile {
        model: "gpt-6.1-sol".into(),
        credential_ref: provider["credentialRef"].as_str().ok_or("provider")?.into(),
        workspace_root: workspace.into(),
        reasoning_effort: "low".into(),
        web_search: false,
        programming: Programming {
            enabled: true,
            ..Default::default()
        },
    };
    eprintln!("qualification: save native model and three policies");
    native
        .execute(local(
            LocalAction::SaveAgentModel,
            json!({"profile":profile}),
        ))
        .await?;
    let policy = Policy {
        budget: Budget {
            limit: Limit::Tokens(100000),
            period: Period::Lifetime,
        },
        requests: RequestPolicy::Allow,
        high_risk: ToolPolicy::AllowWithRules {
            tools: vec![
                "matrix.receive_file".into(),
                "matrix.send_file".into(),
                "codex.command".into(),
                "codex.file_change".into(),
            ],
            directories: vec![workspace.into()],
        },
    };
    native
        .execute(local(
            LocalAction::SaveAgentPolicy,
            json!({"expectedRevision":0,"policy":policy}),
        ))
        .await?;
    for layer in ["room", "requester"] {
        native.execute(local(LocalAction::SavePolicy,json!({"bindingId":binding,"requester":"@alice:test","layer":layer,"expectedRevision":0,"policy":policy}))).await?;
    }
    let input: Vec<u8> = (0..131073).map(|n| n as u8).collect();
    let encrypted = sdk
        .upload_encrypted_file(&mut std::io::Cursor::new(input.clone()))
        .await?;
    let filename = if std::env::var("HAGENCY_ENCRYPTED_NEW_TASK").as_deref() == Ok("1") {
        "encrypted-output-2.bin"
    } else {
        "encrypted-output.bin"
    };
    let prompt = "This is a new independent request. Use list_received_files and receive_file to receive the attached binary. Use the available terminal to read its exact bytes and write encrypted-output.bin in the workspace with exactly those bytes followed by the UTF-8 bytes of \nENCRYPTED_CODEX_OUTPUT\n. Use send_file to send encrypted-output.bin with filename encrypted-output.bin. Observe get_file_delivery until that original delivery is sent; never send a replacement. Then reply ENCRYPTED_CHAIN_COMPLETE. Do actual tools, not just a plan.";
    let prompt = prompt.replace("encrypted-output.bin", filename);
    let content = json!({"msgtype":"m.file","body":prompt,"filename":"encrypted-input.bin","file":encrypted,"info":{"size":input.len(),"mimetype":"application/octet-stream"}});
    let raw = matrix_sdk::ruma::serde::Raw::from_json_string(serde_json::to_string(&content)?)?;
    let started_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    let request = room
        .send_raw("m.room.message", raw)
        .require_encryption()
        .await?;
    std::fs::write(
        root.join("input-event-public.json"),
        serde_json::to_vec_pretty(
            &json!({"event":request.response.event_id,"room":room.room_id(),"agent":agent,"identity":identity}),
        )?,
    )?;
    eprintln!("qualification: start CURRENT runtime for original encrypted attachment");
    let start=native.execute(local(LocalAction::RuntimeStart,json!({"bindingId":binding,"reservation":20000,"effort":"low","hostFiles":false,"takeover":false}))).await?;
    std::fs::write(
        root.join("start-public.json"),
        serde_json::to_vec_pretty(&start)?,
    )?;
    let result=async {
        let http=sdk.http_client();let mut found_file=None;let mut found_reply=None;
        let until=tokio::time::Instant::now()+Duration::from_secs(600);
        loop {
            sdk.sync_once(matrix_sdk::config::SyncSettings::default().timeout(Duration::from_millis(10))).await?;
            let url=format!("{}{}_matrix/client/v3/rooms/{}/messages?dir=b&limit=50",source.inner.origin,"",url::form_urlencoded::byte_serialize(room.room_id().as_str().as_bytes()).collect::<String>());
            let history:Value=serde_json::from_slice(&http.get(url).bearer_auth(sdk.access_token().ok_or("token")?).send().await?.error_for_status()?.bytes().await?)?;
            for event in history["chunk"].as_array().ok_or("room events")? {
                if event["sender"]!=identity["matrixUserId"]||event["type"]!="m.room.encrypted"||event["origin_server_ts"].as_u64().is_none_or(|ts|ts<started_ms){continue;}
                let raw=matrix_sdk::ruma::serde::Raw::from_json_string(serde_json::to_string(event)?)?;
                let Ok(plain)=sdk.encryption().decrypt_received_room_event(room.room_id(),&raw).await else{continue};
                let plain:Value=serde_json::from_str(plain.raw().json().get())?;
                if plain["content"]["msgtype"]=="m.file" {found_file=Some((event.clone(),plain));}
                else if plain["content"]["body"].is_string(){found_reply=Some((event.clone(),plain));}
            }
            let runtime=native.execute(local(LocalAction::Runtime,json!({}))).await?;
            std::fs::write(root.join("runtime-public.json"),serde_json::to_vec_pretty(&runtime)?)?;
            if let Some((_,reply))=&found_reply {
                std::fs::write(root.join("observed-reply-private.json"),serde_json::to_vec_pretty(reply)?)?;
                if !reply["content"]["body"].as_str().is_some_and(|s|s.contains("ENCRYPTED_CHAIN_COMPLETE")){return Err("original model returned a failure instead of completing file delivery".into());}
            }
            if let(Some(file),Some(reply))=(&found_file,&found_reply){
                let attachment=hagency::native_owner::AgentAttachment::from_content(&file.1["content"])?;
                let bytes=source.fetch_agent_attachment(attachment).await?;
                let mut expected=input.clone();expected.extend_from_slice(b"\nENCRYPTED_CODEX_OUTPUT\n");
                if bytes!=expected{return Err("actual decrypted generated file differs".into());}
                if std::fs::read(Path::new(workspace).join(filename))?!=expected{return Err("actual Codex workspace differs".into());}
                std::fs::write(root.join("output-ciphertext-public.json"),serde_json::to_vec_pretty(&file.0)?)?;
                std::fs::write(root.join("reply-ciphertext-public.json"),serde_json::to_vec_pretty(&reply.0)?)?;
                std::fs::write(root.join("decrypted-output-private.json"),serde_json::to_vec_pretty(&json!({"file":file.1,"reply":reply.1}))?)?;
                return Ok::<_,Box<dyn std::error::Error>>(json!({"actualCodex":true,"encryptedInput":request.response.event_id,"encryptedReply":reply.0["event_id"],"encryptedFile":file.0["event_id"],"exactBytes":bytes.len(),"originalCurrentRuntime":true}));
            }
            if tokio::time::Instant::now()>=until{return Err(format!("original task observation timeout: {}",runtime).into());}
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }.await;
    native
        .execute(local(
            LocalAction::RuntimeStop,
            json!({"bindingId":binding}),
        ))
        .await?;
    let result = result?;
    std::fs::write(
        root.join("result.json"),
        serde_json::to_vec_pretty(&result)?,
    )?;
    Ok(())
}
async fn verify_restart(
    native: &NativeOwner,
    sdk: &Client,
    agent: &str,
    direct: &Value,
    room: &matrix_sdk::Room,
    evidence: &Path,
) -> Result<()> {
    use hagency::native_owner::{LocalAction, UsageAction};
    use hagency_agent_local::usage::{Query, Group};
    let original: Value = serde_json::from_slice(&std::fs::read(evidence.join("result.json"))?)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    let query = Query {
        agent: agent.into(),
        binding: None,
        requester: None,
        from: now / 86400 * 86400,
        until: now / 86400 * 86400 + 86400,
        group: Group::Day,
        alerts_before: None,
    };
    let report = |query| Command::AgentUsage {
        action: UsageAction::Report { query },
    };
    let before = native.execute(report(query.clone())).await?;
    let binding = direct["ownerDirect"]["bindingId"]
        .as_str()
        .ok_or("original binding")?;
    let local = |action, input| Command::Local {
        agent_id: agent.into(),
        action,
        input: Some(input),
        binding_id: None,
        requester: None,
    };
    native.execute(local(LocalAction::RuntimeStart,json!({"bindingId":binding,"reservation":20000,"effort":"low","hostFiles":false,"takeover":false}))).await?;
    let mut active = false;
    for _ in 0..8 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let state = native
            .execute(local(LocalAction::Runtime, json!({})))
            .await?;
        active |= state["runtime"]["phase"] == "online";
    }
    native
        .execute(local(
            LocalAction::RuntimeStop,
            json!({"bindingId":binding}),
        ))
        .await?;
    if !active {
        return Err("reopened original service did not become online".into());
    }
    let after = native.execute(report(query)).await?;
    if before["totals"] != after["totals"]
        || after["totals"]["pendingCalls"] != 0
        || after["totals"]["unreportedCalls"] != 0
        || after["totals"]["reserved"] != 0
    {
        return Err("restart changed original costs or started new work".into());
    }
    let mut checked = Vec::new();
    for key in ["encryptedFile", "encryptedReply"] {
        let event = original[key].as_str().ok_or("original event")?;
        let mut url = sdk.homeserver();
        url.path_segments_mut()
            .map_err(|_| "origin")?
            .clear()
            .extend([
                "_matrix",
                "client",
                "v3",
                "rooms",
                room.room_id().as_str(),
                "event",
                event,
            ]);
        let raw: Value = serde_json::from_slice(
            &sdk.http_client()
                .get(url)
                .bearer_auth(sdk.access_token().ok_or("SDK token")?)
                .send()
                .await?
                .error_for_status()?
                .bytes()
                .await?,
        )?;
        let raw = matrix_sdk::ruma::serde::Raw::from_json_string(serde_json::to_string(&raw)?)?;
        sdk.encryption()
            .decrypt_received_room_event(room.room_id(), &raw)
            .await?;
        checked.push(event.to_owned());
    }
    let mut default_encrypted_room = None;
    if std::env::var("HAGENCY_ENCRYPTED_CHECK_NEW_ROOM").as_deref() == Ok("1") {
        let agent = create(native, "encrypted-default-room-probe").await?;
        settled(native, &agent).await?;
        let until = tokio::time::Instant::now() + Duration::from_secs(60);
        let room = loop {
            let value = native
                .execute(Command::EnsureAgentOwnerDirect {
                    agent_id: agent.clone(),
                })
                .await?;
            if value["ownerDirect"]["state"] == "active" {
                break value["ownerDirectRoomId"]
                    .as_str()
                    .ok_or("new encrypted room")?
                    .to_owned();
            }
            if tokio::time::Instant::now() >= until {
                return Err("new default encrypted Room not active".into());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
        let mut url = sdk.homeserver();
        url.path_segments_mut()
            .map_err(|_| "origin")?
            .clear()
            .extend([
                "_matrix",
                "client",
                "v3",
                "rooms",
                &room,
                "state",
                "m.room.encryption",
            ]);
        let encryption: Value = serde_json::from_slice(
            &sdk.http_client()
                .get(url)
                .bearer_auth(sdk.access_token().ok_or("SDK token")?)
                .send()
                .await?
                .error_for_status()?
                .bytes()
                .await?,
        )?;
        if encryption["algorithm"] != "m.megolm.v1.aes-sha2" {
            return Err("new normal owner DM is not encrypted".into());
        }
        default_encrypted_room = Some(room);
    }
    std::fs::write(
        evidence.join("restart-public.json"),
        serde_json::to_vec_pretty(
            &json!({"currentServiceRestarted":true,"noNewUsage":true,"originalCiphertextsStillDecrypt":checked,"usage":after["totals"],"newDefaultEncryptedRoom":default_encrypted_room}),
        )?,
    )?;
    Ok(())
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
