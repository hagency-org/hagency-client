//! The owner console's links must be packaged and served by the real binary.
//! Qualification uses HAGENCY_NATIVE_CONSOLE_ASSETS from build:native.

use serde_json::Value;
use std::{
    net::{SocketAddr, TcpListener as StdListener},
    path::PathBuf,
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

/// The built bundle, resolved to its actual host path. The bundle may be
/// spelled through a symlinked ancestor (this host's `.../home/hl` ->
/// `hl.noindex`); the in-process fixtures canonicalize for the same reason.
fn built() -> PathBuf {
    std::env::var_os("HAGENCY_NATIVE_CONSOLE_ASSETS")
        .map(PathBuf::from)
        .expect("native console qualification requires HAGENCY_NATIVE_CONSOLE_ASSETS from build:native; not a skipped test")
        .canonicalize()
        .expect("native console assets must resolve to an actual host path")
}
fn address() -> SocketAddr {
    StdListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

/// Derive the actual owner navigation instead of retaining legacy Fleet routes.
fn rail_pages() -> Vec<String> {
    let source = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../mockup/components/OwnerRail.jsx"),
    )
    .expect("OwnerRail.jsx must exist");
    let pages: Vec<String> = source
        .split("href=\"")
        .skip(1)
        .map(|tail| {
            let href = tail.split('"').next().expect("static owner href");
            assert!(
                href.starts_with('/') && !href.starts_with("//"),
                "owner navigation must stay local"
            );
            format!("/console{href}")
        })
        .collect();
    assert!(!pages.is_empty(), "owner navigation must expose real pages");
    assert_eq!(
        pages
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        pages.len(),
        "duplicate owner navigation"
    );
    pages
}

/// The bundle path that backs a served page URL.
fn document(path: &str) -> String {
    if path == "/console/" {
        "index.html".to_owned()
    } else {
        format!("{}index.html", path.trim_start_matches("/console/"))
    }
}

/// One GET over a raw socket. The asset routes need no session (they serve
/// under the browser boundary, which checks the Host header and the same-origin
/// fetch site), so a plain request is what a browser document navigation sends.
async fn status(address: SocketAddr, path: &str) -> u16 {
    let mut socket = tokio::net::TcpStream::connect(address)
        .await
        .unwrap_or_else(|error| panic!("connect for {path}: {error}"));
    let request = format!(
        "GET {path} HTTP/1.1\r\nhost: {address}\r\nsec-fetch-site: same-origin\r\nconnection: close\r\n\r\n"
    );
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut wire = Vec::new();
    socket.read_to_end(&mut wire).await.unwrap();
    let head = String::from_utf8_lossy(&wire);
    let line = head.lines().next().unwrap_or_default();
    line.split_whitespace()
        .nth(1)
        .unwrap_or_else(|| panic!("no status line for {path}: {line}"))
        .parse()
        .unwrap_or_else(|_| panic!("unparseable status for {path}: {line}"))
}

/// The reason a spawned executable refused to start — its exit status and its
/// stderr. A generic "process exited" line is the failure mode #83 needed a
/// re-run for, so the refusal names itself.
async fn refusal(server: &mut tokio::process::Child) -> String {
    let mut stderr = String::new();
    if let Some(mut pipe) = server.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr).await;
    }
    format!(
        "exited before admission: {:?}; stderr:\n{stderr}",
        server.try_wait()
    )
}

#[tokio::test]
async fn native_console_rail_pages_are_shipped_and_served() {
    let bundle = built();

    // --- the build script's half: every rail page is IN the bundle ---
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(bundle.join("manifest.json")).expect("the bundle must carry manifest.json"),
    )
    .expect("manifest.json must be JSON");
    let packaged: Vec<&str> = manifest["assets"]
        .as_array()
        .expect("manifest assets")
        .iter()
        .filter_map(|asset| asset["path"].as_str())
        .collect();
    assert!(
        packaged.contains(&"index.html"),
        "the front door is not packaged; packed documents: {packaged:?}"
    );
    for legacy in [
        "resources/index.html",
        "agents/index.html",
        "project-sides/index.html",
        "usage/index.html",
        "approvals/index.html",
        "task-graphs/index.html",
    ] {
        assert!(
            !packaged.contains(&legacy),
            "owner bundle must not expose legacy document {legacy}"
        );
    }
    for page in rail_pages() {
        let document = document(&page);
        assert!(
            packaged.contains(&document.as_str()),
            "the rail links {page} but the build did not package {document}; packed documents: {packaged:?}"
        );
    }

    // --- the service's half: the real binary serves each of them ---
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let state = root.path().join("state");
    let empty_path = root.path().join("empty-path");
    std::fs::create_dir(&empty_path).unwrap();
    let binary = env!("CARGO_BIN_EXE_hagency");
    let init = Command::new(binary)
        .args(["init", "--state-dir"])
        .arg(&state)
        .env_clear()
        .env("PATH", &empty_path)
        .output()
        .await
        .unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let address = address();
    let mut server = Command::new(binary)
        .args(["serve", "--state-dir"])
        .arg(&state)
        .args(["--listen", &address.to_string(), "--console-assets"])
        .arg(&bundle)
        .env_clear()
        .env("PATH", &empty_path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if tokio::net::TcpStream::connect(address).await.is_ok() {
                break;
            }
            assert!(
                server.try_wait().unwrap().is_none(),
                "{}",
                refusal(&mut server).await
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("native executable startup");

    let mut failures = Vec::new();
    for path in std::iter::once("/console/".to_owned()).chain(rail_pages()) {
        let code = status(address, &path).await;
        if code != 200 {
            failures.push(format!("{path} -> {code}"));
        }
    }
    server.kill().await.ok();
    server.wait().await.ok();

    assert!(
        failures.is_empty(),
        "every rail page and the front door must serve: {failures:?}"
    );
}
