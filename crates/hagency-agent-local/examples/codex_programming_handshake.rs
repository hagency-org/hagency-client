//! Actual installed Codex programming configuration and MCP handshake, without inference.
use hagency_agent_local::{
    Ledger, Scope,
    codex::{ExecutionGate, McpServer, Profile, Programming},
};
use std::{path::PathBuf, sync::Arc};
struct Current;
impl ExecutionGate for Current {
    fn authorize<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        Box::pin(async { true })
    }
}
fn mcp_fixture() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{BufRead, Write};
    for line in std::io::stdin().lock().lines() {
        let frame: serde_json::Value = serde_json::from_str(&line?)?;
        if frame.get("id").is_none() {
            continue;
        }
        let result = match frame["method"].as_str() {
            Some("initialize") => {
                serde_json::json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"programming-handshake","version":"1"}})
            }
            Some("tools/list") => {
                serde_json::json!({"tools":[{"name":"probe","description":"Return a fixed non-sensitive response","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}]})
            }
            Some("tools/call") => {
                if let Some(path) = std::env::args_os().nth(2) {
                    std::fs::write(path, b"mcp-ok\n")?;
                }
                serde_json::json!({"content":[{"type":"text","text":"programming MCP probe"}]})
            }
            _ => serde_json::json!({}),
        };
        println!(
            "{}",
            serde_json::json!({"jsonrpc":"2.0","id":frame["id"],"result":result})
        );
        std::io::stdout().flush()?;
    }
    Ok(())
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("guardian")) {
        hagency_platform::run_guardian()?;
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("mcp-fixture") {
        return mcp_fixture();
    }
    let infer = std::env::args_os().nth(2).map(PathBuf::from);
    let binary = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("absolute Codex binary required")?,
    )
    .canonicalize()?;
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?;
    for name in ["home", "provider", "workspace"] {
        std::fs::create_dir(root.join(name))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.join(name), std::fs::Permissions::from_mode(0o700))?;
        }
    }
    let mut programming = Programming {
        enabled: true,
        ..Default::default()
    };
    programming.mcp_servers.insert(
        "probe".into(),
        McpServer::Stdio {
            command: std::env::current_exe()?
                .canonicalize()?
                .to_str()
                .ok_or("executable encoding")?
                .into(),
            args: vec![
                "mcp-fixture".into(),
                root.join("workspace/mcp.txt")
                    .to_str()
                    .ok_or("fixture path")?
                    .into(),
            ],
        },
    );
    let profile = Profile {
        executable: binary,
        home: root.join("home"),
        codex_home: infer
            .as_ref()
            .map(|v| v.canonicalize())
            .transpose()?
            .unwrap_or_else(|| root.join("provider")),
        cwd: root.join("workspace"),
        model: "gpt-6.1-sol".into(),
        effort: "low".into(),
        shared_auth: infer.is_some(),
        web_search: false,
        programming,
    };
    let scope = Scope {
        agent: "probe".into(),
        binding: "probe-binding".into(),
        room: "!probe:test".into(),
        requester: "@owner:test".into(),
        thread: "main".into(),
    };
    let mut ledger = Ledger::open(root.join("ledger"), "@owner:test")?;
    ledger.register_binding("@owner:test", &scope)?;
    for _ in 0..2 {
        let mut process = profile
            .spawn_with_guardian(&std::env::current_exe()?)
            .await?;
        process
            .session
            .enable_programming(Arc::new(Current), "probe-execution".into())?;
        let result = async {
            process.session.initialize().await?;
            process
                .session
                .open_context(&mut ledger, &scope, &profile)
                .await
        }
        .await;
        process.stop().await?;
        result?;
    }
    if infer.is_some() {
        use hagency_agent_local::{Layer, Policy, RequestPolicy, ToolPolicy};
        for layer in [Layer::Agent, Layer::Room, Layer::Requester] {
            ledger.set_policy(
                "@owner:test",
                &scope,
                layer,
                0,
                &Policy {
                    budget: hagency_agent_local::Budget {
                        limit: hagency_agent_local::Limit::Unlimited,
                        period: hagency_agent_local::Period::Lifetime,
                    },
                    requests: RequestPolicy::Allow,
                    high_risk: ToolPolicy::AskOwner,
                },
            )?;
        }
        std::fs::write(profile.cwd.join("existing.txt"), b"before\n")?;
        let mut process = profile
            .spawn_with_guardian(&std::env::current_exe()?)
            .await?;
        process
            .session
            .enable_programming(Arc::new(Current), "acceptance-execution".into())?;
        process.session.initialize().await?;
        process.session.require_local_account().await?;
        process
            .session
            .open_context(&mut ledger, &scope, &profile)
            .await?;
        let (queue, mut pending) = hagency_agent_local::codex::approval_queue(1)?;
        let completed = {
            let owner = async {
                let mut approvals = 0;
                while let Some(proposal) = pending.recv().await {
                    let allow = proposal.proposal.scope == scope
                        && ["codex.command", "codex.file_change", "mcp.probe.probe"]
                            .contains(&proposal.proposal.tool.as_str())
                        && std::path::Path::new(&proposal.proposal.canonical_directory)
                            .starts_with(&profile.cwd);
                    println!(
                        "owner fixture: tool={} approved={allow}",
                        proposal.proposal.tool
                    );
                    proposal.decide(allow);
                    approvals += 1;
                }
                approvals
            };
            let work=process.session.run(&mut ledger,"@owner:test",&scope,"acceptance-call","acceptance-dispatch","In this disposable workspace, use apply_patch to change existing.txt from before to after (with a trailing newline). Use the native shell tool to create command.txt containing exactly command-ok followed by a newline. Call the configured MCP probe tool once. Read the three files and verify their bytes. Work only here and do not inspect provider configuration or credentials. Reply ACCEPTANCE_COMPLETE after verification.",hagency_agent_local::codex::BudgetMode::Estimated{reservation:20000},&queue);
            tokio::pin!(work);
            tokio::pin!(owner);
            let completed = tokio::select! {result=&mut work=>result,_=&mut owner=>return Err("approval queue ended early".into())};
            completed
        };
        process.stop().await?;
        let completed = completed?;
        if !matches!(
            completed.status,
            hagency_agent_local::codex::TurnStatus::Completed
        ) {
            return Err("actual model turn did not complete".into());
        }
        println!(
            "model result: {:?}; tools={}/{}; {}",
            completed.status, completed.tool_returns, completed.tool_requests, completed.text
        );
        for (name, expected) in [
            ("existing.txt", b"after\n".as_slice()),
            ("command.txt", b"command-ok\n".as_slice()),
            ("mcp.txt", b"mcp-ok\n".as_slice()),
        ] {
            if std::fs::read(profile.cwd.join(name))? != expected {
                eprintln!("mismatched fixture file: {name}");
                return Err("actual programming effect mismatch".into());
            }
        }
        let history = ledger.approval_history("@owner:test", &scope.agent, None)?;
        if history.records.is_empty() || history.next_cursor.is_some() {
            return Err("missing or excessive programming audit evidence".into());
        }
        for record in &history.records {
            let evidence = ledger.approval_record("@owner:test", &scope.agent, &record.id)?;
            println!(
                "audit: tool={} phase={:?} events={}",
                record.tool,
                record.phase,
                evidence.events.len()
            );
            if record.phase != hagency_agent_local::audit::Phase::Completed
                || !evidence
                    .events
                    .iter()
                    .any(|e| e.phase == hagency_agent_local::audit::Phase::OwnerAllowed)
                || !evidence
                    .events
                    .iter()
                    .any(|e| e.phase == hagency_agent_local::audit::Phase::Submitted)
            {
                return Err(
                    "programming effect lacks exact approval/submission/completion evidence".into(),
                );
            }
        }
        println!(
            "PASS: real model edited existing file, executed native shell, called explicit MCP, passed owner approvals and stopped whole process tree; status={:?}, input={}, output={}, tools={}/{}",
            completed.status,
            completed.usage.input,
            completed.usage.output,
            completed.tool_returns,
            completed.tool_requests
        );
    }
    if infer.is_none() {
        println!(
            "PASS: actual Codex programming features, sealed config, configured MCP startup, separate context resume and whole-tree stop; no model turn or fees"
        );
    }
    Ok(())
}
