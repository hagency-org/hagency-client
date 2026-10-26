//! Explicit paid qualification through the public NativeOwner facade. The
//! external current Server fixture owns loopback Pasion/Matrix doubles only.
//! Use a fresh isolated database/state. Does not read or copy provider secrets.
use hagency::native_owner::{
    self, Command, HistoryAction, LocalAction, MatrixAccessToken, MatrixTokenSource, NativeError,
    NativeOwner, ProviderAction, UsageAction, UsageGroup, UsageQuery,
};
use hagency_agent_local::{
    Budget, Limit, ModelProfile, Period, Policy, RequestPolicy, ToolPolicy, codex::Programming,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
struct Source {
    origin: String,
}
impl MatrixTokenSource for Source {
    fn access_token(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>> + Send + '_>,
    > {
        Box::pin(async {
            Ok(MatrixAccessToken {
                access_token: "fixture-sdk-only".into(),
                client_id: "sdk-client".into(),
            })
        })
    }
    fn supports_agent_file_receive(&self) -> bool {
        true
    }
    fn fetch_agent_attachment(
        &self,
        attachment: native_owner::AgentAttachment,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<u8>, NativeError>> + Send + '_>,
    > {
        Box::pin(async move {
            attachment.validate().map_err(|_| NativeError {
                status: 400,
                code: "fixture_original_invalid".into(),
            })?;
            if attachment.source["url"] != "mxc://test/joint-incoming" {
                return Err(NativeError {
                    status: 403,
                    code: "fixture_original_required".into(),
                });
            }
            let result = async {
                let response = reqwest::Client::new()
                    .get(format!(
                        "{}_matrix/client/v1/media/download/test/joint-incoming",
                        self.origin
                    ))
                    .bearer_auth("fixture-sdk-only")
                    .send()
                    .await?
                    .error_for_status()?;
                response.bytes().await
            }
            .await;
            result.map(|b| b.to_vec()).map_err(|_| NativeError {
                status: 503,
                code: "fixture_media_unavailable".into(),
            })
        })
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The same executable must implement guardian mode before entering any
    // owner/client initialization. A libtest binary cannot provide this entry.
    if let Some(result) = native_owner::run_guardian_if_requested() {
        result?;
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("HAGENCY_JOINT_REAL_CODEX").as_deref() != Ok("1") {
        return Err("explicit paid inference opt-in required".into());
    }
    let info_path =
        PathBuf::from(std::env::var_os("HAGENCY_FILE_FIXTURE_INFO").ok_or("fixture info")?)
            .canonicalize()?;
    let info: Value = serde_json::from_slice(&std::fs::read(&info_path)?)?;
    let origin = info["origin"].as_str().ok_or("fixture origin")?.to_owned();
    let url: reqwest::Url = origin.parse()?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || info["owner"] != "@alice:test"
        || info["room"] != "!joint-files:test"
    {
        return Err("explicit loopback file fixture required".into());
    }
    let state = PathBuf::from(
        std::env::var_os("HAGENCY_FILE_QUALIFICATION_STATE")
            .ok_or("fresh state directory required")?,
    );
    let resume = std::env::var("HAGENCY_FILE_QUALIFICATION_VERIFY_EXISTING").as_deref() == Ok("1");
    if resume {
        if !state.is_dir()
            || state.parent().ok_or("state parent")?.canonicalize()?
                != info_path.parent().ok_or("fixture parent")?
        {
            return Err(
                "existing validation requires the original isolated fixture directory".into(),
            );
        }
    } else if state.exists() {
        return Err("qualification cannot overwrite existing state".into());
    }
    let native = NativeOwner::open_with_matrix(
        &state,
        &origin,
        "@alice:test",
        Arc::new(Source {
            origin: origin.clone(),
        }),
    )
    .await?;
    let agent = info["agent"]["id"].as_str().ok_or("agent")?.to_owned();
    let binding = info["binding"]["id"].as_str().ok_or("binding")?.to_owned();
    let local = |action, input, binding_id, requester| Command::Local {
        agent_id: agent.clone(),
        action,
        input: Some(input),
        binding_id,
        requester,
    };
    let snapshot = if resume {
        // Validation-only recovery after a driver failure: stop the original
        // intent, inspect original receipts and usage. No provider launch,
        // model input, lease acquisition, prepare, upload or replacement send.
        native
            .execute(local(
                LocalAction::RuntimeStop,
                json!({"bindingId":binding}),
                Some(binding.clone()),
                None,
            ))
            .await?;
        let model = native
            .execute(Command::Local {
                agent_id: agent.clone(),
                action: LocalAction::AgentModel,
                input: None,
                binding_id: None,
                requester: None,
            })
            .await?;
        if model["modelProfile"]["model"] != "gpt-6.1-sol"
            || model["modelProfile"]["programming"]["enabled"] != true
        {
            return Err("original real coding profile required".into());
        }
        reqwest::get(format!("{origin}fixture/snapshot"))
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?
    } else {
        native
            .execute(Command::AssignAgentToCurrentDevice {
                agent_id: agent.clone(),
                expected_generation: info["agent"]["generation"].as_i64().ok_or("generation")?,
            })
            .await?;
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
            .ok_or("prepared workspace")?
            .to_owned();
        let model = ModelProfile {
            model: "gpt-6.1-sol".into(),
            credential_ref: provider["credentialRef"]
                .as_str()
                .ok_or("provider reference")?
                .into(),
            workspace_root: workspace.clone(),
            reasoning_effort: "low".into(),
            web_search: false,
            programming: Programming {
                enabled: true,
                ..Default::default()
            },
        };
        native
            .execute(local(
                LocalAction::SaveAgentModel,
                json!({"profile":model}),
                None,
                None,
            ))
            .await?;
        let policy = Policy {
            budget: Budget {
                limit: Limit::Tokens(100_000),
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
                directories: vec![workspace],
            },
        };
        native
            .execute(local(
                LocalAction::SaveAgentPolicy,
                json!({"expectedRevision":0,"policy":policy}),
                None,
                None,
            ))
            .await?;
        for layer in ["room", "requester"] {
            native.execute(local(LocalAction::SavePolicy,json!({"bindingId":binding,"requester":"@bob:test","layer":layer,"expectedRevision":0,"policy":policy}),Some(binding.clone()),Some("@bob:test".into()))).await?;
        }
        let start = json!({"bindingId":binding,"reservation":20000,"effort":"low","hostFiles":false,"takeover":false});
        native
            .execute(local(
                LocalAction::RuntimeStart,
                start,
                Some(binding.clone()),
                None,
            ))
            .await?;
        reqwest::Client::new()
            .post(format!("{origin}fixture/inbound"))
            .json(&json!({"loseQueueAck":false}))
            .send()
            .await?
            .error_for_status()?;
        let result = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let snapshot: Value = reqwest::get(format!("{origin}fixture/snapshot"))
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                if snapshot["messages"]
                    .as_array()
                    .ok_or("fixture messages")?
                    .iter()
                    .any(|m| {
                        m["room_id"] == info["room"]
                            && m["content"]["body"]
                                .as_str()
                                .is_some_and(|s| s.contains("JOINT_FILES_COMPLETE"))
                    })
                {
                    break Ok::<Value, Box<dyn std::error::Error>>(snapshot);
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        })
        .await;
        let status = native
            .execute(local(
                LocalAction::Runtime,
                json!({}),
                Some(binding.clone()),
                None,
            ))
            .await;
        native
            .execute(local(
                LocalAction::RuntimeStop,
                json!({"bindingId":binding}),
                Some(binding.clone()),
                None,
            ))
            .await?;
        match result {
            Ok(value) => value?,
            Err(e) => {
                eprintln!("original public runtime status: {status:?}");
                return Err(e.into());
            }
        }
    };
    let files: Vec<_> = snapshot["messages"]
        .as_array()
        .ok_or("messages")?
        .iter()
        .filter(|m| m["content"]["msgtype"] == "m.file")
        .collect();
    if files.len() != 1 || snapshot["uploads"] != 1 {
        return Err("exactly one original upload/file required".into());
    }
    let file = files[0];
    if file["sender"] != info["agent"]["puppetMxid"]
        || file["room_id"] != info["room"]
        || file["content"]["m.relates_to"]["event_id"] != info["thread"]
    {
        return Err("original sender/Room/thread required".into());
    }
    let id = file["content"]["url"]
        .as_str()
        .and_then(|s| s.strip_prefix("mxc://test/"))
        .ok_or("fixture media reference")?;
    let bytes = reqwest::Client::new()
        .get(format!(
            "{origin}_matrix/client/v1/media/download/test/{id}"
        ))
        .bearer_auth("fixture-sdk-only")
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let mut expected: Vec<u8> = (0..131_073).map(|n| n as u8).collect();
    expected.extend_from_slice(b"\nfixture output\n");
    if bytes.as_ref() != expected {
        return Err("actual model output bytes differ".into());
    }
    let page = native
        .execute(Command::AgentHistory {
            action: HistoryAction::Files {
                agent: agent.clone(),
                cursor: None,
            },
        })
        .await?;
    let id = page["files"]["records"][0]["record"]["id"]
        .as_str()
        .ok_or("original local file")?
        .to_owned();
    let refreshed = native
        .execute(Command::AgentHistory {
            action: HistoryAction::RefreshFile {
                agent: agent.clone(),
                id,
            },
        })
        .await?;
    let record = &refreshed["file"]["record"];
    let approvals = native
        .execute(Command::AgentHistory {
            action: HistoryAction::Approvals {
                agent: agent.clone(),
                cursor: None,
            },
        })
        .await?;
    let audits = approvals["approvals"]["records"]
        .as_array()
        .ok_or("original approvals")?;
    for tool in ["matrix.receive_file", "matrix.send_file", "codex.command"] {
        if !audits
            .iter()
            .any(|a| a["tool"] == tool && a["phase"] == "completed")
        {
            return Err("matching actual tool completion audit required".into());
        }
    }
    if record["remote"]["state"] != "sent" || record["remote"]["matrixEventId"] != file["event_id"]
    {
        return Err("original delivered receipt required".into());
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let from = now / 86400 * 86400;
    let report = native
        .execute(Command::AgentUsage {
            action: UsageAction::Report {
                query: UsageQuery {
                    agent,
                    binding: None,
                    requester: None,
                    from,
                    until: from + 86400,
                    group: UsageGroup::Day,
                    alerts_before: None,
                },
            },
        })
        .await?;
    if report["totals"]["input"].as_u64().unwrap_or(0) == 0
        || report["totals"]["pendingCalls"] != 0
        || report["totals"]["unreportedCalls"] != 0
        || report["totals"]["reserved"] != 0
    {
        return Err("actual settled usage required".into());
    }
    let evidence = json!({"realCodex":true,"verifiedExisting":resume,"approvals":audits,"matrixAndPasion":"explicit loopback fixtures","fileBytes":bytes.len(),"matrixEventId":file["event_id"],"usage":report["totals"],"file":record});
    std::fs::write(
        state
            .parent()
            .ok_or("state parent")?
            .join("real-codex-files-result.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    println!(
        "Real Codex/public NativeOwner/current Server file qualification passed;131089 exact output bytes; usage={}",
        report["totals"]
    );
    native.shutdown_checked().await?;
    Ok(())
}
