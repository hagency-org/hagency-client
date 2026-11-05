//! Exercise the real private-socket client against a bounded handle registry.
//! Authority is covered by the server tests; this fixture observes client
//! lifetime messages, including failed and cancelled requests.
use super::*;
use crate::daemon::{ipc, protocol};
use std::{collections::HashMap, os::unix::fs::PermissionsExt};

#[derive(Default)]
struct Handles {
    created: usize,
    active: HashMap<String, (String, u64)>,
    retired: Vec<String>,
    blocked: bool,
    requests: Vec<String>,
    mutation_error: Option<String>,
}

struct Fixture {
    _home: tempfile::TempDir,
    controller: Arc<ControllerClient>,
    handles: Arc<Mutex<Handles>>,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    fn start() -> Self {
        // macOS Unix sockets have a short path limit.
        let home = tempfile::Builder::new()
            .prefix("hm")
            .tempdir_in("/tmp")
            .unwrap();
        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let listener = ipc::bind(home.path()).unwrap();
        let handles = Arc::new(Mutex::new(Handles::default()));
        let shared = handles.clone();
        let server = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    incoming = listener.accept() => {
                        let (mut stream, _) = incoming.unwrap();
                        let handles = shared.clone();
                        connections.spawn(async move {
                            let bytes = protocol::read_frame(&mut stream).await.unwrap();
                            let request: protocol::Request = serde_json::from_slice(&bytes).unwrap();
                            assert_eq!(request.owner_id.as_deref(), Some("@owner:test"));
                            let protocol::Command::Bridge { call } = request.command else { panic!("unexpected command") };
                            let (result, blocked): (Result<Value, Failure>, bool) = {
                                let mut state = handles.lock().unwrap();
                                state.requests.push(request.request_id.clone());
                                match call {
                                    Call::Attach { .. } => (Ok(encode(Lease { id: "controller".into(), generation: 1 }).unwrap()), false),
                                    Call::Management { controller, package } => {
                                        if state.active.len() >= 64 {
                                            (Err(Failure::from(error(503, "daemon_bridge_busy"))), false)
                                        } else {
                                            state.created += 1;
                                            let id = format!("manager-{}", state.created);
                                            state.active.insert(id.clone(), (controller, package.instance));
                                            (Ok(Value::String(id)), false)
                                        }
                                    }
                                    Call::Retire { controller, management } => {
                                        let (expected, _) = state.active.remove(&management).expect("retire exactly once");
                                        assert_eq!(controller, expected, "cleanup must use the issuing controller");
                                        state.retired.push(management);
                                        (Ok(Value::Null), false)
                                    }
                                    Call::Invoke { controller, management, .. } => {
                                        let (expected, behavior) = state.active.get(&management).expect("retained handle stays live").clone();
                                        assert_eq!(controller, expected);
                                        if let Some(code)=state.mutation_error.clone() {
                                            (Err(Failure::from(error(503,&code))),false)
                                        } else if behavior == 99 {
                                            (Err(Failure::from(error(403, "fixture_denied"))), false)
                                        } else {
                                            state.blocked = behavior == 100;
                                            (Ok(Value::Null), behavior == 100)
                                        }
                                    }
                                    _ => panic!("unexpected lifecycle call"),
                                }
                            };
                            if blocked {
                                std::future::pending::<()>().await;
                            }
                            let reply = protocol::Reply::ok(&request.request_id, 0, encode(result).unwrap());
                            protocol::write_reply(&mut stream, &serde_json::to_vec(&reply).unwrap()).await.unwrap();
                        });
                    }
                    result = connections.join_next(), if !connections.is_empty() => {
                        result.unwrap().unwrap();
                    }
                }
            }
        });
        Self {
            controller: ControllerClient::new(
                home.path().to_owned(),
                "@owner:test".into(),
                "https://test/".into(),
                1,
            ),
            _home: home,
            handles,
            server,
        }
    }

    async fn management(&self, behavior: u64) -> Arc<ManagementClient> {
        self.controller
            .management_package(Package {
                digest: "a".repeat(64),
                instance: behavior,
                account_epoch: 1,
                permissions: vec![],
                expires_at_ms: None,
            })
            .await
            .unwrap()
    }

    async fn wait(&self, expected: impl Fn(&Handles) -> bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !expected(&self.handles.lock().unwrap()) {
                assert!(!self.server.is_finished(), "socket fixture failed");
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("remote handle cleanup did not arrive");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn poll(manager: &ManagementClient) -> Result<Value, NativeError> {
    let ManagementClient::Remote(remote) = manager else {
        panic!("remote fixture")
    };
    remote
        .invoke(
            Operation::central,
            serde_json::json!([{"operation": "session"}]),
        )
        .await
}

#[tokio::test]
async fn temporary_managers_do_not_fill_daemon_while_a_clone_is_retained() {
    let fixture = Fixture::start();
    let original = fixture.management(1).await;
    let retained = original.clone();
    drop(original);
    for i in 0..96 {
        let behavior = if i % 2 == 0 { 1 } else { 99 };
        let temporary = fixture.management(behavior).await;
        assert_eq!(poll(&temporary).await.is_ok(), behavior == 1);
        drop(temporary);
        fixture
            .wait(|state| state.active.len() == 1 && state.retired.len() == i + 1)
            .await;
        poll(&retained).await.unwrap();
    }
    assert_eq!(fixture.handles.lock().unwrap().created, 97);
    drop(retained);
    fixture
        .wait(|state| state.active.is_empty() && state.retired.len() == 97)
        .await;
}

#[tokio::test]
async fn final_drop_finishes_contended_retirement_and_explicit_retire_is_idempotent() {
    let fixture = Fixture::start();
    let manager = fixture.management(1).await;
    manager.retire();
    manager.retire();
    assert!(manager.is_retired());
    assert_eq!(
        poll(&manager).await.unwrap_err().code,
        "management_scope_changed"
    );
    drop(manager);
    fixture
        .wait(|state| state.active.is_empty() && state.retired.len() == 1)
        .await;

    let manager = fixture.management(1).await;
    let ManagementClient::Remote(remote) = manager.as_ref() else {
        panic!("remote fixture")
    };
    let lock = remote.session.lock().await;
    manager.retire();
    drop(lock);
    // A stale cached controller must not cause cleanup to reattach or use a
    // different controller. The manager remembers its original lease.
    fixture.controller.lease.lock().await.as_mut().unwrap().0.id = "new-controller".into();
    drop(manager);
    fixture
        .wait(|state| state.active.is_empty() && state.retired.len() == 2)
        .await;
}

#[tokio::test]
async fn cancelling_a_poll_retires_its_last_manager_reference() {
    let fixture = Fixture::start();
    let manager = fixture.management(100).await;
    let pending = tokio::spawn(async move { poll(&manager).await });
    fixture.wait(|state| state.blocked).await;
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    fixture
        .wait(|state| state.active.is_empty() && state.retired.len() == 1)
        .await;
}

#[tokio::test]
async fn unreachable_daemon_signout_persists_owner_fence_and_wrong_scope_is_refused() {
    let home=tempfile::tempdir().unwrap();
    crate::daemon::enrollment::Enrollment::enroll(home.path(),"https://test","@owner:test",None,&home.path().join("owners/root")).unwrap();
    let client=ControllerClient::new(home.path().into(),"@owner:test".into(),"https://test/".into(),1);
    tokio::time::timeout(Duration::from_secs(3),client.sign_out()).await.unwrap().unwrap();
    let request=crate::daemon::revocation::load(home.path()).unwrap().unwrap();
    assert_eq!(request.owner,"@owner:test");
    let other=ControllerClient::new(home.path().into(),"@other:test".into(),"https://test/".into(),1);
    assert!(other.sign_out().await.is_err());
    assert_eq!(crate::daemon::revocation::load(home.path()).unwrap().unwrap(),request);
}

#[tokio::test]
async fn explicit_recovery_reopens_expired_original_but_an_ordinary_mutation_does_not() {
    let fixture=Fixture::start();let manager=fixture.management(1).await;
    let ManagementClient::Remote(remote)=manager.as_ref() else {panic!("remote")};
    let (lease,management)=remote.session().await.unwrap();
    let operation=Operation::retry_saved_command;let arguments=serde_json::json!(["native-original"]);
    let call=Call::Invoke{controller:lease.id.clone(),management:management.clone(),operation,arguments:arguments.clone()};
    let key=fingerprint("@owner:test",&remote.package.digest,&call).unwrap();
    let journal=fixture.controller.journal().unwrap();
    let id=journal.pending_for("@owner:test",&key).unwrap();
    let db=rusqlite::Connection::open(fixture._home.path().join("controllers/pending.sqlite3")).unwrap();
    db.execute("UPDATE pending SET retry_until_ms=1",[]).unwrap();
    remote.invoke(operation,arguments).await.unwrap();
    assert_eq!(fixture.handles.lock().unwrap().requests.last(),Some(&id));
    let call=Call::Invoke{controller:lease.id,management,operation:Operation::rename_resource,arguments:serde_json::json!([{"commandId":"new-name"}])};
    let key=fingerprint("@owner:test",&remote.package.digest,&call).unwrap();
    let original=journal.pending_for("@owner:test",&key).unwrap();
    db.execute("UPDATE pending SET retry_until_ms=1",[]).unwrap();
    let before=fixture.handles.lock().unwrap().requests.len();
    assert_eq!(fixture.controller.call(&remote.package.digest,call,true).await.unwrap_err().code,"daemon_command_receipt_expired");
    assert_eq!(fixture.handles.lock().unwrap().requests.len(),before);
    assert_eq!(journal.pending_commands("@owner:test").unwrap()[0].request_id,original);
}

#[tokio::test]
async fn unknown_call_keeps_original_id_through_stopping_scope_busy_and_unknown_gates() {
    let fixture=Fixture::start();let manager=fixture.management(1).await;
    let ManagementClient::Remote(remote)=manager.as_ref() else {panic!("remote")};
    let arguments=serde_json::json!(["original-native-id"]);
    let mut original=None;
    for code in ["daemon_command_pending","daemon_stopping","management_scope_changed","daemon_bridge_busy","future_unknown_gate"] {
        fixture.handles.lock().unwrap().mutation_error=Some(code.into());
        assert_eq!(remote.invoke(Operation::retry_saved_command,arguments.clone()).await.unwrap_err().code,code);
        let id=fixture.handles.lock().unwrap().requests.last().unwrap().clone();
        assert_eq!(original.get_or_insert(id.clone()),&id);
        assert_eq!(fixture.controller.pending_commands().unwrap()[0].request_id,id);
    }
    fixture.handles.lock().unwrap().mutation_error=None;
    remote.invoke(Operation::retry_saved_command,arguments).await.unwrap();
    assert_eq!(fixture.handles.lock().unwrap().requests.last(),original.as_ref());
    assert!(fixture.controller.pending_commands().unwrap().is_empty());
}
