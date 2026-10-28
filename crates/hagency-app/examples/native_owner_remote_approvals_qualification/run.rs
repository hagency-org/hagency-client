use super::source::Source;
use hagency::native_owner::{
    Command, HistoryAction, LocalAction, MatrixTokenSource, NativeOwner, ProviderAction,
    RemoteApprovalAction, UsageAction, UsageGroup, UsageQuery,
};
use hagency_agent_local::{
    Budget, Limit, ModelProfile, Period, Policy, RequestPolicy, ToolPolicy, codex::Programming,
};
use matrix_sdk::{
    Client,
    config::SyncSettings,
    ruma::{api::client::uiaa, UserId},
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
const OWNER: &str = "@alice:test";
const COMMAND: &str = "printf '%s\\n' 'remote approval original bytes' > remote-approved.txt";
const BYTES: &[u8] = b"remote approval original bytes\n";
const MARKER: &str = "REMOTE_APPROVAL_NATIVE_COMPLETE";
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
async fn sdk(origin: &str, path: &Path) -> Result<Client> {
    private_dir(path)?;
    Ok(Client::builder()
        .homeserver_url(origin)
        .http_client(
            matrix_sdk::reqwest::Client::builder()
                .no_proxy()
                .redirect(matrix_sdk::reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .build()?,
        )
        .sqlite_store(path, None)
        .build()
        .await?)
}
async fn sync(client: &Client) -> Result<()> {
    client
        .sync_once(SyncSettings::default().timeout(Duration::from_millis(10)))
        .await?;
    Ok(())
}
async fn call(
    source: &Source,
    method: &str,
    segments: &[&str],
    token: Option<&str>,
    body: Option<Value>,
) -> Result<Value> {
    source.check()?;
    let mut url: url::Url = source.origin.parse()?;
    url.path_segments_mut()
        .map_err(|_| "origin")?
        .clear()
        .extend(segments);
    let mut r = source
        .client
        .http_client()
        .request(method.parse()?, url)
        .bearer_auth(
            token
                .map(str::to_owned)
                .or_else(|| source.client.access_token())
                .ok_or("SDK token")?,
        );
    if let Some(body) = body {
        r = r
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&body)?);
    }
    let response = r.send().await?;
    let status = response.status();
    let value: Value = serde_json::from_slice(&response.bytes().await?)?;
    if !status.is_success() {
        return Err(format!("isolated HTTP {status}: {}", value["errcode"]).into());
    }
    Ok(value)
}
pub async fn run() -> Result<()> {
    if std::env::var("HAGENCY_JOINT_REAL_CODEX").as_deref() != Ok("1") {
        return Err("explicit paid qualification opt-in required".into());
    }
    let bootstrap_path =
        PathBuf::from(std::env::var_os("HAGENCY_SYNAPSE_BOOTSTRAP").ok_or("private bootstrap")?)
            .canonicalize()?;
    let bootstrap: Value = serde_json::from_slice(&std::fs::read(&bootstrap_path)?)?;
    let info: Value =
        serde_json::from_slice(&std::fs::read(std::env::var("HAGENCY_FILE_FIXTURE_INFO")?)?)?;
    let origin = info["origin"].as_str().ok_or("origin")?.to_owned();
    let url: url::Url = origin.parse()?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || info["realHomeserver"] != true
        || bootstrap["owner"]["user_id"] != OWNER
    {
        return Err("isolated actual Synapse fixture required".into());
    }
    let root = PathBuf::from(
        std::env::var_os("HAGENCY_REMOTE_APPROVAL_ROOT").ok_or("fresh qualification directory")?,
    );
    if root.parent().ok_or("root parent")?.canonicalize()?
        != bootstrap_path.parent().ok_or("bootstrap parent")?
    {
        return Err("qualification root must belong to this private fixture".into());
    }
    private_dir(&root)?;
    let host = sdk(&origin, &root.join("host-sdk")).await?;
    host.matrix_auth()
        .restore_session(
            serde_json::from_value(bootstrap["owner"].clone())?,
            Default::default(),
        )
        .await?;
    let password = std::fs::read_to_string(
        bootstrap_path
            .parent()
            .ok_or("bootstrap parent")?
            .join("owner-password.txt"),
    )?;
    eprintln!("qualification: actual SDK cross-signing bootstrap");
    if let Err(error) = host.encryption().bootstrap_cross_signing(None).await {
        let response = error.as_uiaa_response().ok_or(error.to_string())?;
        let mut auth = uiaa::Password::new(
            uiaa::UserIdentifier::Matrix(uiaa::MatrixUserIdentifier::new("alice".into())),
            password.clone(),
        );
        auth.session = response.session.clone();
        host.encryption()
            .bootstrap_cross_signing(Some(uiaa::AuthData::Password(auth)))
            .await?;
    }
    sync(&host).await?;
    let remote = sdk(&origin, &root.join("remote-sdk")).await?;
    // A normal SDK password login follows Synapse's advertised base URL, which
    // bypasses this explicit cohosted OAuth bridge. Restore the ACTUAL Matrix
    // login response into this fresh SDK instead; keep its homeserver pinned to
    // the same loopback proxy as both NativeOwners, just like the host bootstrap.
    let login=host.http_client().post(format!("{origin}_matrix/client/v3/login")).header("content-type","application/json").body(serde_json::to_vec(&json!({"type":"m.login.password","identifier":{"type":"m.id.user","user":"alice"},"password":password,"device_id":"REMOTE-APPROVER"}))?).send().await?.error_for_status()?.bytes().await?;
    remote
        .matrix_auth()
        .restore_session(serde_json::from_slice(&login)?, Default::default())
        .await?;
    sync(&remote).await?;
    let user = UserId::parse(OWNER)?;
    host.encryption().request_user_identity(&user).await?;
    host.encryption()
        .get_device(&user, remote.device_id().ok_or("remote device")?)
        .await?
        .ok_or("uploaded remote SDK device")?
        .verify()
        .await?;
    let host_identity = host
        .encryption()
        .request_user_identity(&user)
        .await?
        .ok_or("host identity")?;
    let remote_identity = remote
        .encryption()
        .request_user_identity(&user)
        .await?
        .ok_or("remote identity")?;
    let master = host_identity
        .master_key()
        .get_first_key()
        .ok_or("host public master")?
        .to_base64();
    if remote_identity
        .master_key()
        .get_first_key()
        .map(|k| k.to_base64())
        .as_deref()
        != Some(master.as_str())
    {
        return Err("independent SDK owner master keys differ".into());
    }
    // Explicit verification of this isolated owner's matching public master.
    // No local-trust flag, imported Olm session or synthesized SDK metadata.
    remote_identity.verify().await?;
    remote.encryption().request_user_identity(&user).await?;
    let host = Arc::new(Source::new(host, &origin)?);
    let remote = Arc::new(Source::new(remote, &origin)?);
    for (name, source) in [("host", &host), ("remote", &remote)] {
        std::fs::write(
            root.join(format!("{name}-session.json")),
            serde_json::to_vec(
                &source
                    .client
                    .matrix_auth()
                    .session()
                    .ok_or("actual SDK session")?,
            )?,
        )?;
    }
    let created=call(&host,"POST",&["_matrix","client","v3","createRoom"],None,Some(json!({"preset":"private_chat","name":"Isolated owner remote approvals","creation_content":{"m.federate":false},"initial_state":[{"type":"m.room.encryption","state_key":"","content":{"algorithm":"m.megolm.v1.aes-sha2"}},{"type":"m.room.history_visibility","state_key":"","content":{"history_visibility":"joined"}},{"type":"m.room.guest_access","state_key":"","content":{"guest_access":"forbidden"}}]}))).await?;
    let approval_room = created["room_id"]
        .as_str()
        .ok_or("approval room")?
        .to_owned();
    sync(&host.client).await?;
    sync(&remote.client).await?;
    host.prepare_remote_approval(&approval_room).await?;
    remote.prepare_remote_approval(&approval_room).await?;
    eprintln!("qualification: actual private SDK preflight passed; opening two NativeOwners");
    let native =
        NativeOwner::open_with_matrix(&root.join("host-owner"), &origin, OWNER, host.clone())
            .await?;
    let reviewer =
        NativeOwner::open_with_matrix(&root.join("remote-owner"), &origin, OWNER, remote.clone())
            .await?;
    let outcome = run_native(
        &native,
        &reviewer,
        &host,
        &remote,
        &approval_room,
        &bootstrap,
        &info,
        &root,
    )
    .await;
    let host_stop = native.shutdown_checked().await;
    let remote_stop = reviewer.shutdown_checked().await;
    host.active
        .store(false, std::sync::atomic::Ordering::Release);
    remote
        .active
        .store(false, std::sync::atomic::Ordering::Release);
    host_stop?;
    remote_stop?;
    outcome
}
async fn run_native(
    native: &NativeOwner,
    reviewer: &NativeOwner,
    host: &Source,
    remote: &Source,
    approval_room: &str,
    bootstrap: &Value,
    info: &Value,
    root: &Path,
) -> Result<()> {
    let agent = info["agent"]["id"].as_str().ok_or("agent")?.to_owned();
    let binding = info["binding"]["id"].as_str().ok_or("binding")?.to_owned();
    let local = |action, input: Option<Value>| Command::Local {
        agent_id: agent.clone(),
        action,
        input,
        binding_id: None,
        requester: None,
    };
    eprintln!("qualification: independent NativeOwner devices");
    let host_devices = native.execute(Command::Devices).await?;
    let remote_devices = reviewer.execute(Command::Devices).await?;
    if host_devices["currentDeviceId"] == remote_devices["currentDeviceId"] {
        return Err("independent NativeOwner execution-device identities required".into());
    }
    native
        .execute(Command::AssignAgentToCurrentDevice {
            agent_id: agent.clone(),
            expected_generation: info["agent"]["generation"]
                .as_i64()
                .ok_or("agent generation")?,
        })
        .await?;
    eprintln!("qualification: local provider reference and original workspace");
    let provider = native
        .execute(Command::Provider {
            action: ProviderAction::UseLocal,
        })
        .await?;
    let prepared = native
        .execute(Command::PrepareAgentWorkspace {
            agent_id: agent.clone(),
        })
        .await?;
    let workspace = prepared["workspace"]
        .as_str()
        .ok_or("workspace")?
        .to_owned();
    let output = PathBuf::from(&workspace).join("remote-approved.txt");
    if output.exists() {
        return Err("fresh original workspace required".into());
    }
    let model = ModelProfile {
        model: "gpt-6.1-sol".into(),
        credential_ref: provider["credentialRef"]
            .as_str()
            .ok_or("credential reference")?
            .into(),
        workspace_root: workspace.clone(),
        reasoning_effort: "low".into(),
        web_search: false,
        programming: Programming {
            enabled: true,
            ..Default::default()
        },
    };
    eprintln!("qualification: save Agent model");
    native
        .execute(local(
            LocalAction::SaveAgentModel,
            Some(json!({"profile":model})),
        ))
        .await?;
    let policy = Policy {
        budget: Budget {
            limit: Limit::Tokens(100_000),
            period: Period::Lifetime,
        },
        requests: RequestPolicy::Allow,
        high_risk: ToolPolicy::AskOwner,
    };
    eprintln!("qualification: save three-layer AskOwner policy");
    native
        .execute(local(
            LocalAction::SaveAgentPolicy,
            Some(json!({"expectedRevision":0,"policy":policy})),
        ))
        .await?;
    for layer in ["room", "requester"] {
        native.execute(local(LocalAction::SavePolicy,Some(json!({"bindingId":binding,"requester":"@bob:test","layer":layer,"expectedRevision":0,"policy":policy})))).await?;
    }
    native.execute(local(LocalAction::RuntimeStart,Some(json!({"bindingId":binding,"reservation":20000,"effort":"low","hostFiles":false,"takeover":false})))).await?;
    eprintln!("qualification: original runtime started; submitting one actual Matrix task");
    let task=call(host,"PUT",&["_matrix","client","v3","rooms",info["room"].as_str().ok_or("room")?,"send","m.room.message","remote-original-task"],bootstrap["requester"]["access_token"].as_str(),Some(json!({"msgtype":"m.text","body":format!("{} Run exactly this one shell command, unchanged, using the available terminal tool in the workspace:\n{COMMAND}\nWait for owner approval; do not run other commands or create this file another way. After successful completion report exactly {MARKER}.",info["agent"]["puppetMxid"].as_str().ok_or("puppet")?),"m.mentions":{"user_ids":[info["agent"]["puppetMxid"]]},"m.relates_to":{"rel_type":"m.thread","event_id":info["thread"],"is_falling_back":false}}))).await?;
    let proposal = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let pending = native.execute(local(LocalAction::Approvals, None)).await?;
            if let Some(rows) = pending["approvals"].as_array()
                && !rows.is_empty()
            {
                if rows.len() != 1 {
                    return Err("exactly one original live approval required".into());
                }
                break Ok::<Value, Box<dyn std::error::Error>>(rows[0].clone());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await??;
    if proposal["proposal"]["tool"] != "codex.command"
        || !proposal["proposal"]["arguments"]["command"]
            .as_str()
            .is_some_and(exact_command)
        || proposal["proposal"]["canonical_directory"] != workspace
        || output.exists()
    {
        return Err(format!("original exact pending command required: {proposal}").into());
    }
    let approval = proposal["proposalId"]
        .as_str()
        .ok_or("approval ID")?
        .to_owned();
    native
        .execute(local(
            LocalAction::PublishRemoteApproval {
                approval_id: approval.clone(),
            },
            Some(json!({"argsDigest":proposal["argsDigest"],"approvalRoom":approval_room})),
        ))
        .await?;
    let published = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let status = native
                .execute(local(
                    LocalAction::RemoteApprovalStatus {
                        approval_id: approval.clone(),
                    },
                    None,
                ))
                .await?;
            if status["remoteApproval"]["phase"] == "sent" {
                break Ok::<Value, Box<dyn std::error::Error>>(status);
            }
            if status["remoteApproval"]["phase"] == "unknown" {
                return Err("original request publication unknown; no retry".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await??;
    sync(&remote.client).await?;
    let page = reviewer
        .execute(Command::RemoteApprovals {
            action: RemoteApprovalAction::List {
                agent: agent.clone(),
                room: approval_room.into(),
                cursor: None,
            },
        })
        .await?;
    let original = page["requests"]
        .as_array()
        .ok_or("remote inbox")?
        .iter()
        .find(|r| r["request"]["approvalId"] == approval)
        .ok_or("actual verified remote request")?;
    if original["request"]["proposal"] != proposal["proposal"]
        || original["requestEventId"] != published["remoteApproval"]["requestEventId"]
        || output.exists()
    {
        return Err("actual remote request differs or execution ran before approval".into());
    }
    reviewer
        .execute(Command::RemoteApprovals {
            action: RemoteApprovalAction::Decide {
                agent: agent.clone(),
                room: approval_room.into(),
                event_id: original["requestEventId"]
                    .as_str()
                    .ok_or("request event")?
                    .into(),
                request_digest: original["requestDigest"]
                    .as_str()
                    .ok_or("request digest")?
                    .into(),
                approved: true,
            },
        })
        .await?;
    let sent = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let status = reviewer
                .execute(Command::RemoteApprovals {
                    action: RemoteApprovalAction::Status {
                        approval_id: approval.clone(),
                    },
                })
                .await?;
            if status["publication"]["phase"] == "sent" {
                break Ok::<Value, Box<dyn std::error::Error>>(status);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await??;
    if output.exists() {
        return Err("remote publication must not bypass original pending consumption".into());
    }
    sync(&host.client).await?;
    native
        .execute(local(
            LocalAction::ObserveRemoteApproval {
                approval_id: approval.clone(),
            },
            Some(json!({"argsDigest":proposal["argsDigest"]})),
        ))
        .await?;
    eprintln!("qualification: verified remote verdict consumed by original live PendingApproval");
    let snapshot = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let response = host
                .client
                .http_client()
                .get(format!("{}fixture/snapshot", host.origin))
                .send()
                .await?
                .error_for_status()?
                .bytes()
                .await?;
            let response: Value = serde_json::from_slice(&response)?;
            if response["messages"]
                .as_array()
                .ok_or("messages")?
                .iter()
                .any(|m| {
                    m["sender"] == info["agent"]["puppetMxid"]
                        && m["room_id"] == info["room"]
                        && m["content"]["msgtype"] == "m.text"
                        && m["content"]["body"]
                            .as_str()
                            .is_some_and(|s| s.contains(MARKER))
                })
            {
                break Ok::<Value, Box<dyn std::error::Error>>(response);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await??;
    native
        .execute(local(
            LocalAction::RuntimeStop,
            Some(json!({"bindingId":binding})),
        ))
        .await?;
    if std::fs::read(&output)? != BYTES {
        return Err("actual approved command output bytes differ".into());
    }
    let audit = native
        .execute(Command::AgentHistory {
            action: HistoryAction::Approval {
                agent: agent.clone(),
                id: approval.clone(),
            },
        })
        .await?;
    if audit["approval"]["summary"]["phase"] != "completed"
        || !audit["approval"]["events"]
            .as_array()
            .ok_or("approval events")?
            .iter()
            .any(|e| {
                e["phase"] == "owner_allowed"
                    && e["detail"]["origin"] == "verified_matrix_device"
                    && e["detail"]["eventId"] == sent["publication"]["eventId"]
                    && e["detail"]["requestEventId"]
                        == published["remoteApproval"]["requestEventId"]
            })
    {
        return Err(
            "matching verified remote owner decision and actual tool receipt required".into(),
        );
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let usage = native
        .execute(Command::AgentUsage {
            action: UsageAction::Report {
                query: UsageQuery {
                    agent: agent.clone(),
                    binding: None,
                    requester: None,
                    from: now / 86400 * 86400,
                    until: (now / 86400 + 1) * 86400,
                    group: UsageGroup::Day,
                    alerts_before: None,
                },
            },
        })
        .await?;
    if usage["totals"]["input"].as_u64().unwrap_or(0) == 0
        || usage["totals"]["pendingCalls"] != 0
        || usage["totals"]["unreportedCalls"] != 0
        || usage["totals"]["reserved"] != 0
    {
        return Err("actual settled original usage required".into());
    }
    let evidence = json!({"realSynapse":true,"realSdkCrypto":true,"realNativeOwners":true,"realCodex":true,"productionOAuth":false,"manualUi":false,"agent":agent,"taskEventId":task["event_id"],"request":published,"verdict":sent,"approval":audit,"usage":usage["totals"],"bytes":BYTES.len(),"messages":snapshot["messages"]});
    std::fs::write(
        root.join("result.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    println!(
        "Actual Synapse/two SDKs/two NativeOwners/real Codex remote approval finished; usage={}",
        usage["totals"]
    );
    Ok(())
}

fn exact_command(command: &str) -> bool {
    command == COMMAND
        || shlex::split(command).is_some_and(|args| {
            args.len() == 3 && args[0] == "/bin/zsh" && args[1] == "-lc" && args[2] == COMMAND
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn qualification_command_guard_accepts_only_exact_original_shell_payload() {
        assert!(exact_command(COMMAND));
        let wrapped = format!("/bin/zsh -lc {}", shlex::try_quote(COMMAND).unwrap());
        assert!(exact_command(&wrapped));
        for bad in [
            format!("{COMMAND}; touch foreign"),
            format!("{wrapped} extra"),
            format!("{wrapped}; touch foreign"),
            format!("cd elsewhere; {COMMAND}"),
            COMMAND.replace("remote-approved.txt", "different.txt"),
            "/bin/zsh -lc 'unterminated".into(),
        ] {
            assert!(!exact_command(&bad), "{bad}");
        }
    }
}
