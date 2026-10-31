//! Isolated supervised fixture: no credentials, model or production entrypoint.
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, BufRead, Write},
    path::Path,
    time::Duration,
};
fn main() -> io::Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    #[cfg(unix)]
    if args.first().is_some_and(|v| v == "guardian") {
        return hagency_platform::run_guardian();
    }
    let [mode, root, overrides @ ..] = args.as_slice() else {
        return Err(io::Error::other("offline fixture only"));
    };
    let mode = mode.to_string_lossy();
    let root = Path::new(root);
    fs::write(root.join("process-id"), std::process::id().to_string())?;
    let mut quota = 0;
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("methods"))?;
    for line in io::stdin().lock().lines() {
        let v: Value = serde_json::from_str(&line?).map_err(io::Error::other)?;
        let Some(method) = v["method"].as_str() else {
            continue;
        };
        writeln!(log, "{method}")?;
        log.flush()?;
        if method == "initialized" {
            continue;
        }
        if mode.starts_with("text-workspace") && matches!(method, "thread/start" | "turn/start") {
            let name = if method == "thread/start" { "thread-request.json" } else { "turn-request.json" };
            fs::write(root.join(name), serde_json::to_vec(&v["params"])?)?;
        }
        if (mode == "delay" && method == "account/rateLimits/read")
            || (mode == "delay-config" && method == "config/read")
        {
            std::thread::sleep(Duration::from_secs(30));
        }
        if mode == "text-held-initialize" && method == "initialize" {
            let until = std::time::Instant::now() + Duration::from_secs(8);
            while !root.join("release-initialization").exists() && std::time::Instant::now() < until
            {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if mode == "text-held-qualify" && method == "account/rateLimits/read" {
            let until = std::time::Instant::now() + Duration::from_secs(8);
            while !root.join("release-qualification").exists() && std::time::Instant::now() < until
            {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if mode == "slow-quota" && method == "account/rateLimits/read" {
            std::thread::sleep(Duration::from_millis(2200));
        }
        if mode == "request" && method == "config/read" {
            println!(
                "{}",
                json!({"id":"bad-request","method":"item/commandExecution/requestApproval","params":{}})
            );
            io::stdout().flush()?;
        }
        let value = match method {
            "initialize" => {
                json!({"userAgent":"managed-offline-fixture","platformFamily":"unix","platformOs":"fixture","codexHome":"/fixture"})
            }
            "config/read" => {
                let mut config: Value =
                    serde_json::from_slice(&fs::read(root.join("config.json"))?)
                        .map_err(io::Error::other)?;
                for pair in overrides.as_chunks::<2>().0 {
                    if pair[0] != "-c" {
                        return Err(io::Error::other("fixture override"));
                    }
                    let setting = pair[1].to_string_lossy();
                    let (key, value) = setting
                        .split_once('=')
                        .ok_or(io::Error::other("fixture override"))?;
                    if value != "false" {
                        return Err(io::Error::other("fixture override"));
                    }
                    let keys: Vec<_> = key.split('.').collect();
                    match keys.as_slice() {
                        ["features", name] => config["features"][*name] = false.into(),
                        ["mcp_servers", name, "enabled"] => {
                            config["mcp_servers"][*name]["enabled"] = false.into()
                        }
                        _ => return Err(io::Error::other("fixture override")),
                    }
                }
                json!({"config":config})
            }
            "account/read" => {
                json!({"requiresOpenaiAuth":true,"account":{"type":"chatgpt","email":null,"planType":"plus"}})
            }
            "account/rateLimits/read" => {
                quota += 1;
                json!({"accountId":if mode=="changed-account"&&quota>1 {"fixture_changed"}else{"fixture_account"},"rateLimits":{}})
            }
            "thread/start" if mode.starts_with("text-workspace") => {
                let sandbox = match mode.as_ref() {
                    "text-workspace-standard" => json!({"type":"workspaceWrite","writableRoots":[v["params"]["cwd"]],"networkAccess":false}),
                    "text-workspace-yolo" => json!({"type":"dangerFullAccess"}),
                    _ => json!({"type":"readOnly","networkAccess":false}),
                };
                json!({"thread":{"id":"text-thread","cwd":v["params"]["cwd"],"status":{"type":"idle"},"turns":[]},"cwd":v["params"]["cwd"],"model":v["params"]["model"],"modelProvider":"openai","approvalPolicy":v["params"]["approvalPolicy"],"approvalsReviewer":"user","sandbox":sandbox})
            }
            "turn/start" if mode.starts_with("text-workspace") => {
                let thread: Value = serde_json::from_slice(&fs::read(root.join("thread-request.json"))?)?;
                if v["params"]["cwd"] != thread["cwd"] {
                    return Err(io::Error::other("room workspace changed within turn"));
                }
                json!({"turn":{"id":"text-turn","items":[],"status":"inProgress"}})
            }
            "thread/start" if mode.starts_with("text") => {
                if v["params"]["sandbox"] != "read-only"
                    || v["params"]["approvalPolicy"] != "on-request"
                {
                    return Err(io::Error::other("fixture text policy"));
                }
                json!({"thread":{"id":"text-thread","cwd":v["params"]["cwd"],"status":{"type":"idle"},"turns":[]},"cwd":v["params"]["cwd"],"model":v["params"]["model"],"modelProvider":"openai","approvalPolicy":"on-request","approvalsReviewer":"user","sandbox":{"type":"readOnly","networkAccess":false}})
            }
            "turn/start" if mode.starts_with("text") => {
                if v["params"]["sandboxPolicy"]["type"] != "readOnly"
                    || v["params"]["sandboxPolicy"]["networkAccess"] != false
                {
                    return Err(io::Error::other("fixture turn policy"));
                }
                json!({"turn":{"id":"text-turn","items":[],"status":"inProgress"}})
            }
            _ => return Err(io::Error::other("fixture received non-metadata command")),
        };
        println!("{}", json!({"id":v["id"],"result":value}));
        if method == "turn/start" && mode.starts_with("text") {
            io::stdout().flush()?;
            if matches!(&*mode, "text-block" | "text-renew") {
                let deadline = std::time::Instant::now()
                    + std::time::Duration::from_secs(if mode == "text-renew" { 12 } else { 5 });
                while !root.join("release-text").exists() {
                    if std::time::Instant::now() >= deadline {
                        return Err(io::Error::other("fixture release missing"));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
            if mode == "text-web" {
                for (method, timestamp) in [("item/started", "startedAtMs"), ("item/completed", "completedAtMs")] {
                    let mut params = json!({"threadId":"text-thread","turnId":"text-turn","item":{"type":"webSearch","id":"weather-search","query":"Beijing weather"}});
                    params[timestamp] = 1.into();
                    println!("{}", json!({"method":method,"params":params}));
                }
            }
            if mode == "text-context-workspace" {
                let cwd = Path::new(v["params"]["cwd"].as_str().ok_or_else(|| io::Error::other("missing cwd"))?);
                let marker = cwd.join("room-turn-count");
                let previous = fs::read_to_string(&marker).unwrap_or_default().parse::<u64>().unwrap_or(0);
                fs::write(marker, (previous + 1).to_string())?;
                let mut paths = fs::OpenOptions::new().create(true).append(true).open(root.join("room-paths"))?;
                writeln!(paths, "{}", serde_json::to_string(cwd)?)?;
            }
            let answer = if mode.starts_with("text-context") {
                let input = v["params"]["input"][0]["text"]
                    .as_str()
                    .ok_or_else(|| io::Error::other("context input missing"))?;
                let mut inputs = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(root.join("inputs"))?;
                writeln!(
                    inputs,
                    "{}",
                    serde_json::to_string(input).map_err(io::Error::other)?
                )?;
                let (_, value) = input
                    .split_once('\n')
                    .ok_or_else(|| io::Error::other("context prefix missing"))?;
                let value: Value = serde_json::from_str(value).map_err(io::Error::other)?;
                let request = value["currentRequest"]["body"].as_str().unwrap_or_default();
                if let Some(name) = request.strip_prefix("Remember the name ") {
                    format!("The name is {name}")
                } else {
                    value["context"]["history"]
                        .as_array()
                        .and_then(|h| h.last())
                        .and_then(|p| p["delivery"]["body"].as_str())
                        .unwrap_or("No prior delivered name.")
                        .into()
                }
            } else if mode == "text-escaped" {
                "\n".repeat(32 * 1024)
            } else if mode == "text-items" {
                "## 调研结果\n\n**完成**\n\n".to_owned()+&"正文\n\n".repeat(1200)
            } else {
                "Offline answer.".into()
            };
            if matches!(
                &*mode,
                "text-usage" | "text-long" | "text-overflow" | "text-stream" | "text-escaped" | "text-items" | "text-renew" | "text-overrun" | "text-context" | "text-context-workspace"
            ) {
                let usage = if mode == "text-overrun" {
                    json!({"totalTokens":4813,"inputTokens":4713,"cachedInputTokens":0,"cacheWriteInputTokens":0,"outputTokens":100,"reasoningOutputTokens":0})
                } else {
                    json!({"totalTokens":300,"inputTokens":200,"cachedInputTokens":100,"cacheWriteInputTokens":20,"outputTokens":100,"reasoningOutputTokens":0})
                };
                println!(
                    "{}",
                    json!({"method":"thread/tokenUsage/updated","params":{"threadId":"text-thread","turnId":"text-turn","tokenUsage":{"total":usage,"last":usage,"modelContextWindow":10000}}})
                );
            }
            if mode=="text-items" {
                println!("{}",json!({"method":"item/started","params":{"threadId":"text-thread","turnId":"text-turn","startedAtMs":1,"item":{"type":"agentMessage","id":"progress","text":"","phase":"commentary"}}}));
                for delta in ["正在", "调研。"] {
                    println!("{}",json!({"method":"item/agentMessage/delta","params":{"threadId":"text-thread","turnId":"text-turn","itemId":"progress","delta":delta}}));
                }
                println!("{}",json!({"method":"item/completed","params":{"threadId":"text-thread","turnId":"text-turn","completedAtMs":2,"item":{"type":"agentMessage","id":"progress","text":"正在调研。","phase":"commentary"}}}));
            }
            println!(
                "{}",
                json!({"method":"item/started","params":{"threadId":"text-thread","turnId":"text-turn","startedAtMs":1,"item":{"type":"agentMessage","id":"answer","text":"","phase":"final_answer"}}})
            );
            let long_prefix=if mode=="text-long" || mode=="text-overflow" {
                let count=if mode=="text-long" {6000}else{12000};
                for _ in 0..count {
                    println!("{}",json!({"method":"item/agentMessage/delta","params":{"threadId":"text-thread","turnId":"text-turn","itemId":"answer","delta":"研"}}));
                }
                "研".repeat(count)
            }else{String::new()};
            if mode == "text-stream" {
                // Continuous output has no five-second idle gap for a renewal.
                for _ in 0..60 {
                    println!("{}", json!({"method":"item/agentMessage/delta","params":{"threadId":"text-thread","turnId":"text-turn","itemId":"answer","delta":"x"}}));
                    io::stdout().flush()?;
                    std::thread::sleep(std::time::Duration::from_millis(600));
                }
            }
            println!(
                "{}",
                json!({"method":"item/agentMessage/delta","params":{"threadId":"text-thread","turnId":"text-turn","itemId":"answer","delta":answer}})
            );
            let answer = long_prefix + &if mode == "text-stream" { "x".repeat(60) + &answer } else { answer };
            println!(
                "{}",
                json!({"method":"item/completed","params":{"threadId":"text-thread","turnId":"text-turn","completedAtMs":2,"item":{"type":"agentMessage","id":"answer","text":answer,"phase":"final_answer"}}})
            );
            println!(
                "{}",
                json!({"method":"turn/completed","params":{"threadId":"text-thread","turn":{"id":"text-turn","status":"completed","items":[]}}})
            );
        }
        io::stdout().flush()?;
    }
    Ok(())
}
