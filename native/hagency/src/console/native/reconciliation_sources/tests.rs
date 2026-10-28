use super::*;
use hagency_core::project::Resource;
use serde_json::{Value, json};
use std::{
    fs,
    sync::atomic::{AtomicBool, AtomicUsize},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Source {
    live: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
}
impl super::super::MatrixTokenSource for Source {
    fn access_token(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<super::super::MatrixAccessToken, NativeError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async {
            self.calls.fetch_add(1, Ordering::AcqRel);
            if !self.live.load(Ordering::Acquire) {
                return Err(failure(401, "matrix_account_changed"));
            }
            Ok(super::super::MatrixAccessToken {
                access_token: "fixture-sdk".into(),
                client_id: "sdk-client".into(),
            })
        })
    }
}
static TEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
pub(in crate::console::native) struct Fixture {
    _serial: tokio::sync::MutexGuard<'static, ()>,
    pub(in crate::console::native) temp: tempfile::TempDir,
    pub(in crate::console::native) owner: NativeOwner,
    #[cfg(feature = "resource-adoption")]
    pub(in crate::console::native) recipient_routes: Arc<std::sync::Mutex<std::collections::BTreeMap<String,Value>>>,
    #[cfg(feature = "resource-adoption")]
    pub(in crate::console::native) recipient_requests: Arc<std::sync::Mutex<Vec<String>>>,
    origin: String,
    profile: String,
    pub(in crate::console::native) live: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
    network: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    pub(in crate::console::native) async fn new() -> Self { Self::with_installation(false).await }
    #[cfg(feature = "resource-adoption")]
    pub(in crate::console::native) async fn with_configured_installation() -> Self { Self::with_installation(true).await }
    async fn with_installation(configured: bool) -> Self {
        let serial = TEST_SERIAL.lock().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        let server_origin = origin.clone();
        let network = Arc::new(AtomicUsize::new(0));
        let count = network.clone();
        let recipient_routes = Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::<String,Value>::new()));
        let recipient_requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let routes = recipient_routes.clone();
        let requests = recipient_requests.clone();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let origin = server_origin.clone();
                let count = count.clone();
                let routes = routes.clone();
                let requests = requests.clone();
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut part = [0; 4096];
                    let end = loop {
                        let n = stream.read(&mut part).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        raw.extend_from_slice(&part[..n]);
                        assert!(raw.len() < 32768);
                        if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            break i + 4;
                        }
                    };
                    let head = String::from_utf8_lossy(&raw[..end]).to_string();
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.split_once(':')
                                .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                                .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    while raw.len() < end + length {
                        let n = stream.read(&mut part).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        raw.extend_from_slice(&part[..n]);
                    }
                    let path = head
                        .lines()
                        .next()
                        .unwrap()
                        .split_whitespace()
                        .nth(1)
                        .unwrap();
                    count.fetch_add(1, Ordering::AcqRel);
                    let at = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64;
                    requests.lock().unwrap().push(head.lines().next().unwrap().to_owned());
                    let decoded = percent_encoding::percent_decode_str(path).decode_utf8().unwrap().into_owned();
                    let fixture_reply = routes.lock().unwrap().get(&decoded).cloned();
                    let value = if let Some(value) = fixture_reply { value } else { match path {
                        "/hagency/api/v1/discovery" => {
                            json!({"product":"hagency-server","version":"0.1.0","protocolVersion":3,"capabilities":["pasion-oauth","owner-agent-appservice-v1","global-agent-identity-v2","execution-device-v1","owner-direct-v1"],"homeserver":origin,"issuer":format!("{origin}auth/")})
                        }
                        "/hagency/api/v1/sessions/pasion" => {
                            json!({"token":"a".repeat(64),"userId":"uid","mxid":"@owner:test","validUntilMs":at+30000})
                        }
                        "/hagency/api/v1/identity" => {
                            json!({"userId":"uid","mxid":"@owner:test","issuer":format!("{origin}auth/"),"subject":"source-subject","clientId":"sdk-client","validUntilMs":at+30000})
                        }
                        "/hagency/api/v1/devices" => {
                            json!({"deviceId":"fixture-device","token":"b".repeat(64),"generation":1,"validUntilMs":at+30000})
                        }
                        "/hagency/api/v1/sessions/current/renew" => {
                            json!({"validUntilMs":at+30000})
                        }
                        "/hagency/api/v1/devices/current" => {
                            json!({"userId":"uid","mxid":"@owner:test","issuer":format!("{origin}auth/"),"subject":"source-subject","clientId":"sdk-client","deviceId":"fixture-device","generation":1,"validUntilMs":at+30000})
                        }
                        "/hagency/api/v1/sessions/current" => json!({}),
                        "/hagency/api/v1/agents" => json!({"agents":[]}),
                        _ => panic!("unexpected fixture route: {path}"),
                    }};
                    let body = value.to_string();
                    let _=stream.write_all(format!("HTTP/1.1 200 Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await;
                });
            }
        });
        let temp = if configured {
            hagency_platform::legacy_writer::test_support::configured_root().unwrap();
            tempfile::tempdir().unwrap()
        } else { hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap() };
        let live = Arc::new(AtomicBool::new(true));
        let calls = Arc::new(AtomicUsize::new(0));
        let owner = NativeOwner::open_with_matrix(
            &temp.path().join("host"),
            &origin,
            "@owner:test",
            Arc::new(Source {
                live: live.clone(),
                calls: calls.clone(),
            }),
        )
        .await
        .unwrap();
        // Isolate inspection from the owner's independently running legacy
        // configuration worker; this preserves real SDK/device authorization.
        let tasks = owner
            .0
            .configuration_tasks
            .lock()
            .unwrap()
            .drain(..)
            .collect::<Vec<_>>();
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
        let profile = profile_identity(
            &origin,
            &format!("{origin}auth/"),
            "source-subject",
            "@owner:test",
        )
        .unwrap();
        Self {
            _serial: serial,
            temp,
            owner,
            origin,
            profile,
            live,
            calls,
            network,
            #[cfg(feature = "resource-adoption")]
            recipient_routes,
            #[cfg(feature = "resource-adoption")]
            recipient_requests,
            server,
        }
    }
    fn root(&self) -> PathBuf {
        self.owner.0.host.state_directory().into()
    }
    fn request(&self, roots: Vec<PathBuf>) -> SourceInspectionRequest {
        let live = self.live.clone();
        SourceInspectionRequest::new(
            &self.owner,
            package(),
            roots,
            Instant::now() + Duration::from_secs(20),
            move || live.load(Ordering::Acquire),
        )
        .unwrap()
    }
    fn owners(&self) -> PathBuf {
        let path = self.root().join("owned-agent-owners");
        hagency_store::private::directory(&path).unwrap();
        path
    }
    pub(in crate::console::native) fn domain(
        &self,
    ) -> (hagency_store::DomainRepository, PathBuf, Resource) {
        let path = self.owners().join(format!("management_{}", self.profile));
        let mut db = hagency_store::DomainRepository::open_managed(&path, &self.profile).unwrap();
        let resource:Resource=serde_json::from_value(json!({"presetId":"original-preset","seatId":"original-seat","framework":"codex","model":"gpt-5.6-sol","reasoning":"medium","published":false,"ceiling":{"tokens":5000,"period":"monthly"}})).unwrap();
        db.put_resource(&resource).unwrap();
        (db, path, resource)
    }
    pub(in crate::console::native) fn personal(&self) -> (hagency_agent_local::Ledger, PathBuf) {
        let dir = self.owners().join(format!("owner_{}", self.profile));
        hagency_store::private::directory(&dir).unwrap();
        hagency_store::private::replace(&dir.join("profile-identity.json"), &serde_json::to_vec(&json!({"version":1,"origin":self.origin,"issuer":format!("{}auth/",self.origin),"subject":"source-subject","owner":"@owner:test"})).unwrap()).unwrap();
        let path = dir.join("agent-local.sqlite");
        hagency_store::private::open(&path, true).unwrap();
        let ledger =
            hagency_agent_local::Ledger::open_scoped(&path, "@owner:test", &self.profile).unwrap();
        (ledger, path)
    }
    pub(in crate::console::native) async fn stop(self) {
        self.owner.shutdown().await;
        self.server.abort();
    }
}
fn package() -> ManagementPackage {
    ManagementPackage {
        digest: "a".repeat(64),
        instance: 1,
        account_epoch: 1,
        permissions: vec![Permission::ReadResources],
    }
}
fn bytes(path: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut values = Vec::new();
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file()
            && !entry.file_name().to_string_lossy().ends_with("-shm")
        {
            values.push((
                entry.file_name().to_string_lossy().into(),
                fs::read(entry.path()).unwrap(),
            ));
        }
    }
    values.sort();
    values
}

#[tokio::test]
async fn census_missing_sources_are_not_created_or_recovered() {
    let f = Fixture::new().await;
    assert!(!f.root().join("owned-agent-owners").exists());
    let before = bytes(&f.root());
    let network = f.network.load(Ordering::Acquire);
    let calls = f.calls.load(Ordering::Acquire);
    let census = f
        .owner
        .inspect_reconciliation_sources(f.request(vec![]))
        .await
        .unwrap();
    assert_eq!(census.adoption, "not_assessed");
    assert!(!census.external_roots_complete);
    assert_eq!(
        census.roots[0].domain.status,
        SourceObservationStatus::NotPresent
    );
    assert_eq!(
        census.roots[0].personal.status,
        SourceObservationStatus::NotPresent
    );
    assert!(!f.root().join("owned-agent-owners").exists());
    assert_eq!(before, bytes(&f.root()));
    assert_eq!(
        f.network.load(Ordering::Acquire),
        network,
        "inspection does not invoke native grant renewal"
    );
    assert!(
        f.calls.load(Ordering::Acquire) >= calls + 2,
        "SDK epoch checked on both sides"
    );
    f.stop().await;
}

#[tokio::test]
async fn census_preserves_populated_wal_resources_private_receipts_and_uncertain_usage() {
    let f = Fixture::new().await;
    let (_domain, domain_dir, resource) = f.domain();
    let (mut personal_ledger, personal_path) = f.personal();
    personal_ledger
        .register_binding(
            "@owner:test",
            &hagency_agent_local::Scope {
                agent: "private-agent".into(),
                binding: "private-binding".into(),
                room: "!private-room:test".into(),
                requester: "@private-requester:test".into(),
                thread: "private-thread".into(),
            },
        )
        .unwrap();
    let domain = rusqlite::Connection::open(domain_dir.join("domain.sqlite3")).unwrap();
    domain.execute("INSERT INTO management_commands(actor_scope,command_id,operation,digest,result) VALUES ('private-actor','completed-private','resource.admit','secret-digest','{}')",[]).unwrap();
    let personal = rusqlite::Connection::open(&personal_path).unwrap();
    personal.execute("INSERT INTO calls VALUES ('unknown-call','private-binding','private-scope','private-digest',400,'[]','unknown',NULL)",[]).unwrap();
    personal.execute("INSERT INTO calls VALUES ('zero-call','private-binding','private-scope','private-digest',0,'[]','settled',?1)",[json!({"input":0,"output":0,"cached_input":0,"reasoning_output":0,"accounting_version":"unreported-zero-v1"}).to_string()]).unwrap();
    personal
        .execute(
            "INSERT INTO accounts VALUES ('overlapping-private-scope','lifetime',25,400)",
            [],
        )
        .unwrap();
    let before_domain = bytes(&domain_dir);
    let before_personal = bytes(personal_path.parent().unwrap());
    let census = f
        .owner
        .inspect_reconciliation_sources(f.request(vec![]))
        .await
        .unwrap();
    let d = census.roots[0].domain.evidence.as_ref().unwrap();
    assert_eq!(d.history.resource_definitions.observed, 1);
    assert_eq!(d.history.command_receipts.observed, 1);
    assert_eq!(d.resources[0].id, resource.id());
    assert_eq!(
        d.resources[0].revision,
        hagency_store::resource_publication_revision(&resource).unwrap()
    );
    assert_eq!(
        census.roots[0].personal.status,
        SourceObservationStatus::Inspected
    );
    let p = census.roots[0].personal.evidence.as_ref().unwrap();
    assert_eq!(p.history.unknown_calls, 1);
    assert_eq!(p.history.unreported_zero_usage, 1);
    assert_eq!(p.history.positive_held_rows, 1);
    assert_eq!(before_domain, bytes(&domain_dir));
    assert_eq!(before_personal, bytes(personal_path.parent().unwrap()));
    let public = serde_json::to_string(&census).unwrap();
    for secret in [
        "private-",
        "source-subject",
        "original-seat",
        "original-preset",
        f.root().to_str().unwrap(),
    ] {
        assert!(!public.contains(secret));
    }
    drop(personal);
    drop(domain);
    drop(personal_ledger);
    drop(_domain);
    f.stop().await;
}

#[tokio::test]
async fn census_other_profile_contents_are_not_opened_or_disclosed() {
    let f = Fixture::new().await;
    let other = f.owners().join(format!("management_{}", "f".repeat(64)));
    hagency_store::private::directory(&other).unwrap();
    hagency_store::private::replace(
        &other.join("domain.sqlite3"),
        b"private OTHER-PROFILE contents",
    )
    .unwrap();
    let before = bytes(&other);
    let census = f
        .owner
        .inspect_reconciliation_sources(f.request(vec![]))
        .await
        .unwrap();
    assert_eq!(census.roots[0].other_profile_directories, 1);
    assert_eq!(
        census.roots[0].domain.status,
        SourceObservationStatus::NotPresent
    );
    assert_eq!(before, bytes(&other));
    let public = serde_json::to_string(&census).unwrap();
    assert!(!public.contains("OTHER-PROFILE"));
    assert!(!public.contains(&"f".repeat(64)));
    f.stop().await;
}

#[tokio::test]
async fn census_rejects_wrong_profile_malformed_metadata_and_unexpected_schema() {
    let f = Fixture::new().await;
    let (_db, path, _) = f.domain();
    let db = rusqlite::Connection::open(path.join("domain.sqlite3")).unwrap();
    db.execute(
        "UPDATE management_identity SET profile_digest=?1",
        ["d".repeat(64)],
    )
    .unwrap();
    hagency_store::private::replace(&f.root().join("server-login-profiles.json"), b"malformed")
        .unwrap();
    let census = f
        .owner
        .inspect_reconciliation_sources(f.request(vec![]))
        .await
        .unwrap();
    assert_eq!(
        census.roots[0].domain.status,
        SourceObservationStatus::Unsupported
    );
    assert_eq!(
        census.roots[0].profile_metadata,
        SourceObservationStatus::Unsupported
    );
    db.execute(
        "UPDATE management_identity SET profile_digest=?1",
        [&f.profile],
    )
    .unwrap();
    db.pragma_update(None, "user_version", 9999).unwrap();
    let census = f
        .owner
        .inspect_reconciliation_sources(f.request(vec![]))
        .await
        .unwrap();
    assert_eq!(
        census.roots[0].domain.status,
        SourceObservationStatus::Unsupported
    );
    drop(db);
    drop(_db);
    f.stop().await;
}

#[tokio::test]
async fn census_missing_wal_companion_never_creates_one() {
    let f = Fixture::new().await;
    let (db, path, _) = f.domain();
    drop(db);
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(path.join(format!("domain.sqlite3{suffix}")));
    }
    let before = bytes(&path);
    let census = f
        .owner
        .inspect_reconciliation_sources(f.request(vec![]))
        .await
        .unwrap();
    assert_eq!(
        census.roots[0].domain.status,
        SourceObservationStatus::RecoveryRequired
    );
    assert_eq!(before, bytes(&path));
    f.stop().await;
}

#[tokio::test]
async fn census_live_authority_and_package_permissions_are_required() {
    let f = Fixture::new().await;
    let mut denied = package();
    denied.permissions.clear();
    assert!(
        SourceInspectionRequest::new(
            &f.owner,
            denied,
            vec![],
            Instant::now() + Duration::from_secs(10),
            || true
        )
        .is_err()
    );
    assert!(
        SourceInspectionRequest::new(
            &f.owner,
            package(),
            vec![],
            Instant::now() + Duration::from_secs(31),
            || true
        )
        .is_err()
    );
    let request = f.request(vec![]);
    f.live.store(false, Ordering::Release);
    assert!(
        f.owner
            .inspect_reconciliation_sources(request)
            .await
            .is_err()
    );
    assert!(!f.root().join("owned-agent-owners").exists());
    f.stop().await;
}

#[test]
fn census_queued_inspection_is_denied_when_owner_retires() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    rt.block_on(async {
        let f = Fixture::new().await;
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let busy = tokio::task::spawn_blocking(move || {
            entered.send(()).unwrap();
            blocked.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        waiting.await.unwrap();
        let mut audit = Box::pin(f.owner.inspect_reconciliation_sources(f.request(vec![])));
        std::future::poll_fn(|cx| match audit.as_mut().poll(cx) {
            std::task::Poll::Pending => std::task::Poll::Ready(()),
            std::task::Poll::Ready(v) => panic!("read bypassed occupied worker: {v:?}"),
        })
        .await;
        f.owner.retire();
        release.send(()).unwrap();
        busy.await.unwrap();
        assert!(audit.await.is_err());
        assert!(!f.root().join("owned-agent-owners").exists());
        f.stop().await;
    });
}

#[cfg(unix)]
#[tokio::test]
async fn census_symlink_and_replaced_file_are_refused_without_following() {
    let f = Fixture::new().await;
    let (_db, path, _) = f.domain();
    let dir = filesystem::Directory::open(&path).unwrap();
    let held = filesystem::Database::existing(&dir, "domain.sqlite3").unwrap();
    fs::rename(path.join("domain.sqlite3"), path.join("original.sqlite3")).unwrap();
    std::os::unix::fs::symlink(path.join("original.sqlite3"), path.join("domain.sqlite3")).unwrap();
    assert!(held.check().is_err());
    let census = f
        .owner
        .inspect_reconciliation_sources(f.request(vec![]))
        .await
        .unwrap();
    assert_eq!(
        census.roots[0].domain.status,
        SourceObservationStatus::Unreadable
    );
    drop(held);
    drop(_db);
    f.stop().await;
}

#[test]
fn census_root_lock_child() {
    use std::io::{Read, Write};
    let Some(path) = std::env::var_os("HAGENCY_CENSUS_TEST_LOCK") else {
        return;
    };
    let lock = fs::File::open(path).unwrap();
    lock.try_lock().unwrap();
    println!("census-lock-held");
    std::io::stdout().flush().unwrap();
    let mut release = [0];
    std::io::stdin().read_exact(&mut release).unwrap();
}

#[tokio::test]
async fn census_additional_root_busy_in_another_process_is_not_inspected() {
    use std::io::{BufRead, Write};
    use std::process::{Command, Stdio};
    let f = Fixture::new().await;
    let extra = crate::owner_host::initialize(&f.temp.path().join("other-root")).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "console::native::reconciliation_sources::tests::census_root_lock_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("HAGENCY_CENSUS_TEST_LOCK", extra.join(".owner-client.lock"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert!(
            output.read_line(&mut line).unwrap() > 0,
            "child did not hold lock"
        );
        if line.contains("census-lock-held") {
            break;
        }
    }
    let before = bytes(&extra);
    let census = f
        .owner
        .inspect_reconciliation_sources(f.request(vec![extra.clone()]))
        .await
        .unwrap();
    assert_eq!(census.roots[1].status, SourceObservationStatus::Busy);
    assert_eq!(before, bytes(&extra));
    child.stdin.take().unwrap().write_all(&[1]).unwrap();
    assert!(child.wait().unwrap().success());
    let census = f
        .owner
        .inspect_reconciliation_sources(f.request(vec![extra.clone()]))
        .await
        .unwrap();
    assert_eq!(census.roots[1].status, SourceObservationStatus::Inspected);
    assert_eq!(
        census.roots[1].domain.status,
        SourceObservationStatus::NotPresent
    );
    assert_eq!(before, bytes(&extra));
    f.stop().await;
}

#[tokio::test]
async fn census_unknown_history_is_retained_and_duplicate_roots_not_counted_twice() {
    let f = Fixture::new().await;
    let (_domain, path, _) = f.domain();
    let db = rusqlite::Connection::open(path.join("domain.sqlite3")).unwrap();
    db.execute_batch("CREATE TABLE private_unknown_history(value TEXT); INSERT INTO private_unknown_history VALUES ('private-liability');").unwrap();
    let census = f
        .owner
        .inspect_reconciliation_sources(f.request(vec![f.root()]))
        .await
        .unwrap();
    let evidence = census.roots[0].domain.evidence.as_ref().unwrap();
    assert!(evidence.history.other_history_rows.observed > 0);
    assert!(
        evidence
            .blockers
            .contains(&hagency_store::management::AuditBlocker::OtherHistory)
    );
    assert_eq!(census.roots[1].status, SourceObservationStatus::Unsupported);
    assert!(
        !serde_json::to_string(&census)
            .unwrap()
            .contains("private_unknown_history")
    );
    drop(db);
    drop(_domain);
    f.stop().await;
}

#[tokio::test]
async fn census_concurrent_legacy_personal_initialization_is_observation_only() {
    let f = Fixture::new().await;
    let personal = f
        .root()
        .join("owned-agent-owners")
        .join(format!("owner_{}", f.profile))
        .join("agent-local.sqlite");
    assert!(!personal.exists());
    // Explicit legacy callers still initialize this source. The inspector
    // observes their concurrent work without invoking that opener itself.
    let root = f.root();
    let origin = f.origin.clone();
    let initializer = tokio::task::spawn_blocking(move || {
        crate::console::owned_agents::ledger_path(
            &root,
            &origin,
            &format!("{origin}auth/"),
            "source-subject",
            "@owner:test",
        )
        .unwrap()
    });
    let census = f
        .owner
        .inspect_reconciliation_sources(f.request(vec![]))
        .await
        .unwrap();
    assert_eq!(census.adoption, "not_assessed");
    assert!(!census.external_roots_complete);
    assert!(census.roots[0].domain.evidence.is_none());
    assert!(matches!(
        census.roots[0].personal.status,
        SourceObservationStatus::Inspected
            | SourceObservationStatus::NotPresent
            | SourceObservationStatus::Changed
            | SourceObservationStatus::Unreadable
            | SourceObservationStatus::RecoveryRequired
            | SourceObservationStatus::Busy
    ));
    initializer.await.unwrap();
    assert!(
        personal.exists(),
        "the concurrent legacy opener really initialized a source"
    );
    let before = bytes(personal.parent().unwrap());
    let census = f
        .owner
        .inspect_reconciliation_sources(f.request(vec![]))
        .await
        .unwrap();
    assert_eq!(census.adoption, "not_assessed");
    assert_eq!(before, bytes(personal.parent().unwrap()));
    f.stop().await;
}

#[test]
fn census_queued_sdk_source_change_discards_read_only_result() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    rt.block_on(async {
        let f = Fixture::new().await;
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let busy = tokio::task::spawn_blocking(move || {
            entered.send(()).unwrap();
            blocked.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        waiting.await.unwrap();
        let request = SourceInspectionRequest::new(
            &f.owner,
            package(),
            vec![],
            Instant::now() + Duration::from_secs(10),
            || true,
        )
        .unwrap();
        let mut audit = Box::pin(f.owner.inspect_reconciliation_sources(request));
        std::future::poll_fn(|cx| match audit.as_mut().poll(cx) {
            std::task::Poll::Pending => std::task::Poll::Ready(()),
            std::task::Poll::Ready(value) => panic!("inspection bypassed queue: {value:?}"),
        })
        .await;
        f.live.store(false, Ordering::Release);
        release.send(()).unwrap();
        busy.await.unwrap();
        assert!(audit.await.is_err());
        assert!(!f.root().join("owned-agent-owners").exists());
        f.stop().await;
    });
}

#[tokio::test]
async fn census_native_review_cannot_cross_an_owner_instance_even_for_same_account() {
    let f = Fixture::new().await;
    let other = NativeOwner::open_with_matrix(
        &f.temp.path().join("same-account-other-owner"),
        &f.origin,
        "@owner:test",
        Arc::new(Source {
            live: f.live.clone(),
            calls: f.calls.clone(),
        }),
    )
    .await
    .unwrap();
    let before = f.calls.load(Ordering::Acquire);
    assert!(
        other
            .inspect_reconciliation_sources(f.request(vec![]))
            .await
            .is_err()
    );
    assert_eq!(
        f.calls.load(Ordering::Acquire),
        before,
        "wrong owner must fail before SDK/source reads"
    );
    other.shutdown().await;
    f.stop().await;
}

#[tokio::test]
async fn census_legacy_profile_marker_is_observed_without_rewriting_it() {
    let f = Fixture::new().await;
    let (_ledger, path) = f.personal();
    let marker = path.parent().unwrap().join("profile-identity.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
    value["issuer"] = crate::server_admission::profile_namespace_issuer(&f.origin).into();
    hagency_store::private::replace(&marker, &serde_json::to_vec(&value).unwrap()).unwrap();
    let before = bytes(path.parent().unwrap());
    let census = f
        .owner
        .inspect_reconciliation_sources(f.request(vec![]))
        .await
        .unwrap();
    assert_eq!(
        census.roots[0].personal.status,
        SourceObservationStatus::Inspected
    );
    assert_eq!(before, bytes(path.parent().unwrap()));
    drop(_ledger);
    f.stop().await;
}

#[cfg(feature = "resource-adoption")]
impl Fixture {
    pub(in crate::console::native) async fn restart_onboarding(
        self, selection: crate::console::native::AdoptedManagementSelection,
    ) -> Self {
        let Self {_serial,temp,owner,origin,profile,live,calls,network,recipient_routes,recipient_requests,server}=self;
        let state=owner.0.host.state_directory().to_owned();
        let source=owner.0.matrix_source.lock().unwrap().as_ref().unwrap().clone();
        owner.suspend().await.unwrap();drop(owner);
        let owner=selection.open_for_onboarding_with_matrix_guarded(&state,&origin,"@owner:test",source,|_|Ok(())).await.unwrap();
        Self {_serial,temp,owner,origin,profile,live,calls,network,recipient_routes,recipient_requests,server}
    }
    pub(in crate::console::native) async fn restart_adopted(
        self,
        registry: hagency_store::owner_resources::OwnerResourceRegistry,
        migration_id: String,
    ) -> Self {
        let Self {
            _serial,
            temp,
            owner,
            origin,
            profile,
            live,
            calls,
            network,
            recipient_routes,
            recipient_requests,
            server,
        } = self;
        let state = owner.0.host.state_directory().to_owned();
        let source = owner
            .0
            .matrix_source
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .clone();
        owner.suspend().await.unwrap();
        drop(owner);
        let missing = NativeOwner::open_adopted_management_with_matrix_guarded(
            &state,
            &origin,
            "@owner:test",
            source.clone(),
            registry.clone(),
            "missing-original".into(),
            |_| Ok(()),
        )
        .await;
        assert!(missing.is_err());
        drop(missing);
        let owner = NativeOwner::open_adopted_management_with_matrix_guarded(
            &state,
            &origin,
            "@owner:test",
            source,
            registry,
            migration_id,
            |_| Ok(()),
        )
        .await
        .unwrap();
        // Active adopted owners retain only the resource-alert observer.
        assert_eq!(owner.0.configuration_tasks.lock().unwrap().len(), 1);
        Self {
            _serial,
            temp,
            owner,
            origin,
            profile,
            live,
            calls,
            network,
            recipient_routes,
            recipient_requests,
            server,
        }
    }
    pub(in crate::console::native) async fn restart_for_adoption(
        self,
        registry: hagency_store::owner_resources::OwnerResourceRegistry,
        migration_id: String,
    ) -> Self {
        let Self {
            _serial,
            temp,
            owner,
            origin,
            profile,
            live,
            calls,
            network,
            recipient_routes,
            recipient_requests,
            server,
        } = self;
        let state = owner.0.host.state_directory().to_owned();
        let source = owner
            .0
            .matrix_source
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .clone();
        owner.suspend().await.unwrap();
        drop(owner);
        let missing = NativeOwner::open_for_resource_adoption_with_matrix_guarded(
            &state,
            &origin,
            "@owner:test",
            source.clone(),
            registry.clone(),
            "missing-original".into(),
            |_| Ok(()),
        )
        .await;
        assert!(
            missing.is_err(),
            "a missing original canonical migration must fail"
        );
        drop(missing);
        let premature = NativeOwner::open_adopted_management_with_matrix_guarded(
            &state,
            &origin,
            "@owner:test",
            source.clone(),
            registry.clone(),
            migration_id.clone(),
            |_| Ok(()),
        )
        .await;
        assert!(
            premature.is_err(),
            "prepared/fenced source is not active resource authority"
        );
        drop(premature);
        let owner = NativeOwner::open_for_resource_adoption_with_matrix_guarded(
            &state,
            &origin,
            "@owner:test",
            source,
            registry,
            migration_id,
            |_| Ok(()),
        )
        .await
        .unwrap();
        assert!(owner.0.configuration_tasks.lock().unwrap().is_empty());
        Self {
            _serial,
            temp,
            owner,
            origin,
            profile,
            live,
            calls,
            network,
            recipient_routes,
            recipient_requests,
            server,
        }
    }
}

#[tokio::test]
async fn census_recognizes_native_resume_metadata_without_reading_or_importing_credentials() {
    let f = Fixture::new().await;
    let path = f.root().join("server-login-sdk-resume.json");
    let original = std::fs::read(&path).unwrap();
    let census = f
        .owner
        .inspect_reconciliation_sources(f.request(vec![]))
        .await
        .unwrap();
    assert_eq!(census.roots[0].status, SourceObservationStatus::Inspected);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let projection = serde_json::to_string(&census).unwrap();
    assert!(!projection.contains("session_token"));
    assert!(!projection.contains("server-login-sdk-resume"));
    assert!(!census.external_roots_complete);
    f.stop().await;
}
