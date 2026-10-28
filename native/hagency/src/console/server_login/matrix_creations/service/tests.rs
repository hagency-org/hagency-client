use super::*;
use crate::console::server_login::matrix_creations::tests::{command, fixture, input, state};
use serde_json::json;
use std::sync::Arc;

async fn console(root: &Path, login: &ServerLogin) -> (Console, String) {
    let state = root.join("state");
    hagency_store::private::directory(&state).unwrap();
    let console = Console::native_with_state(&state).unwrap();
    let ticket = console.owner_access_ticket().unwrap();
    let cookie = console.0.authority.exchange(&ticket).unwrap();
    let session = login
        .sessions
        .lock()
        .await
        .get(&session_key("cookie"))
        .unwrap()
        .clone();
    console
        .0
        .server_login
        .sessions
        .lock()
        .await
        .insert(session_key(&cookie), session);
    (console, cookie)
}
fn owner_read() -> (&'static str, Value, Option<Value>) {
    ("GET /hagency/api/v1/projects", json!({"projects":[]}), None)
}
fn discovery() -> (&'static str, Value, Option<Value>) {
    (
        "GET /hagency/api/v1/discovery",
        json!({"serverName":"example.test","serviceMxid":"@_hagency_service:example.test"}),
        None,
    )
}
fn space() -> (Command, Value) {
    let mut c = command();
    c.input = input(Kind::Space);
    let mut proof = state(&c);
    proof[0]["content"]["type"] = json!("m.space");
    (c, proof)
}
fn value(reply: Reply) -> Value {
    serde_json::to_value(reply).unwrap()
}
fn expect_error(result: Result<Reply, OwnerError>, code: &str) {
    assert_eq!(result.err().expect("expected refusal").code, code);
}
async fn done(task: tokio::task::JoinHandle<()>) {
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn list_journal_uses_profile_identity_without_authorizing_copied_personal_database() {
    let (login, server) = fixture(vec![owner_read(), owner_read()]).await;
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let (console, cookie) = console(root.path(), &login).await;
    let listed = value(invoke(&console, &cookie, Action::List, None).await.unwrap());
    assert_eq!(listed, json!({"creations":[]}));
    let session = login
        .sessions
        .lock()
        .await
        .get(&session_key("cookie"))
        .unwrap()
        .clone();
    let origin = &session.binding.origin;
    let issuer = format!("{origin}auth/");
    let owner = session.binding.owner.as_deref().unwrap();
    let directory = crate::console::owned_agents::journal_profile_directory(
        &root.path().join("state"),
        origin,
        &issuer,
        "subject",
        owner,
    )
    .unwrap();
    let copied = directory.join("agent-local.sqlite");
    assert!(!copied.exists());
    let foreign = crate::console::owned_agents::ledger_path(
        &root.path().join("state"),
        origin,
        &issuer,
        "another-subject",
        owner,
    )
    .unwrap();
    std::fs::copy(foreign, &copied).unwrap();
    let bytes = std::fs::read(&copied).unwrap();
    assert_eq!(
        value(invoke(&console, &cookie, Action::List, None).await.unwrap()),
        listed
    );
    assert_eq!(std::fs::read(&copied).unwrap(), bytes);
    assert!(
        crate::console::owned_agents::ledger_path(
            &root.path().join("state"),
            origin,
            &issuer,
            "subject",
            owner,
        )
        .is_err()
    );
    done(server).await;
    console.retire();
}
#[tokio::test]
async fn direct_service_preserves_journal_wire_and_replays_without_create() {
    let (c, proof) = space();
    let body = MatrixOperation::Create(
        c.input.clone(),
        c.owner.clone(),
        "@_hagency_service:example.test".into(),
    )
    .request()
    .unwrap()
    .2;
    let (login, server) = fixture(vec![
        owner_read(),
        discovery(),
        (
            "POST /_matrix/client/v3/createRoom",
            json!({"room_id":"!room:example.test"}),
            body,
        ),
        (
            "GET /_matrix/client/v3/rooms/!room:example.test/state",
            proof,
            None,
        ),
        (
            "POST /hagency/api/v1/projects/adopt",
            json!({"project":{"id":"prj_created","spaceId":"!room:example.test"}}),
            Some(json!({"spaceId":"!room:example.test"})),
        ),
        owner_read(),
        owner_read(),
        owner_read(),
        owner_read(),
        owner_read(),
    ])
    .await;
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let (console, cookie) = console(root.path(), &login).await;
    let created = value(
        invoke(&console, &cookie, Action::Start(c.input.clone()), None)
            .await
            .unwrap(),
    );
    assert_eq!(created["creation"]["phase"], "complete");
    for action in [
        Action::Start(c.input.clone()),
        Action::Status(c.input.command_id.clone()),
        Action::Resume {
            id: c.input.command_id.clone(),
            input: Resume::default(),
        },
    ] {
        assert_eq!(
            value(invoke(&console, &cookie, action, None).await.unwrap()),
            created
        );
    }
    let list = value(invoke(&console, &cookie, Action::List, None).await.unwrap());
    assert_eq!(
        list,
        json!({"creations":[{"input":c.input,"creation":created["creation"]}]})
    );
    let mut changed = c.input.clone();
    changed.name = "Different".into();
    expect_error(
        invoke(&console, &cookie, Action::Start(changed), None).await,
        "creation_command_conflict",
    );
    done(server).await;
    let session = login
        .sessions
        .lock()
        .await
        .get(&session_key("cookie"))
        .unwrap()
        .clone();
    let directory = crate::console::owned_agents::journal_profile_directory(
        &root.path().join("state"),
        &session.binding.origin,
        &format!("{}auth/", session.binding.origin),
        "subject",
        &c.owner,
    )
    .unwrap();
    assert!(!directory.join("agent-local.sqlite").exists());
    let bytes =
        hagency_store::private::read_secret(&directory.join("matrix-creations.json")).unwrap();
    let persisted: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        persisted[0],
        json!({"input":c.input,"origin":session.binding.origin,"owner":c.owner,
        "subject":"subject","roomId":"!room:example.test","phase":"complete",
        "project":{"id":"prj_created","spaceId":"!room:example.test"},"lastError":null})
    );
    assert!(!String::from_utf8(bytes).unwrap().contains("owner-session"));
    console.retire();
}
#[tokio::test]
async fn uncertain_create_survives_restart_and_only_recovers_exact_marked_room() {
    let (c, proof) = space();
    let (login, server) = fixture(vec![
        owner_read(),
        discovery(),
        // Accepted POST with a lost/malformed result is uncertain, not safe to retry.
        ("POST /_matrix/client/v3/createRoom", json!({}), None),
        owner_read(),
        owner_read(),
        (
            "GET /_matrix/client/v3/rooms/!room:example.test/state",
            proof.clone(),
            None,
        ),
        (
            "GET /_matrix/client/v3/rooms/!room:example.test/state",
            proof,
            None,
        ),
        (
            "POST /hagency/api/v1/projects/adopt",
            json!({"project":{"id":"prj_created"}}),
            None,
        ),
    ])
    .await;
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let (first, cookie) = console(root.path(), &login).await;
    let uncertain = value(
        invoke(&first, &cookie, Action::Start(c.input.clone()), None)
            .await
            .unwrap(),
    );
    assert_eq!(uncertain["creation"]["phase"], "unknown");
    first.retire();
    drop(first);
    let (restored, cookie) = console(root.path(), &login).await;
    assert_eq!(
        value(
            invoke(&restored, &cookie, Action::Start(c.input.clone()), None)
                .await
                .unwrap()
        ),
        uncertain
    );
    let recovered = value(
        invoke(
            &restored,
            &cookie,
            Action::Resume {
                id: c.input.command_id,
                input: Resume {
                    room_id: Some("!room:example.test".into()),
                },
            },
            None,
        )
        .await
        .unwrap(),
    );
    assert_eq!(recovered["creation"]["phase"], "complete");
    assert!(recovered["creation"]["lastError"].is_null());
    done(server).await;
    restored.retire();
}
#[tokio::test]
async fn waiting_for_journal_never_returns_after_local_authority_revocation() {
    let (login, server) = fixture(vec![owner_read()]).await;
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let (console, cookie) = console(root.path(), &login).await;
    let console = Arc::new(console);
    let scope = console.0.authority.authenticate(&cookie).unwrap();
    let guard = COMMANDS.lock().await;
    let work = tokio::spawn({
        let console = console.clone();
        let cookie = cookie.clone();
        async move { invoke(&console, &cookie, Action::List, None).await }
    });
    done(server).await;
    console.0.authority.revoke(&scope).unwrap();
    drop(guard);
    expect_error(work.await.unwrap(), "sign_in_required");
    console.retire();
}
#[tokio::test]
async fn scope_refuses_each_identity_change_before_any_matrix_or_owner_write() {
    for field in [
        "origin",
        "owner",
        "subject",
        "client",
        "installation",
        "grant",
    ] {
        let (login, server) = fixture(vec![owner_read()]).await;
        let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
        let (console, cookie) = console(root.path(), &login).await;
        let scope = Scope::new(&console, &cookie).await.unwrap();
        done(server).await;
        {
            let mut sessions = console.0.server_login.sessions.lock().await;
            let s = sessions.get_mut(&session_key(&cookie)).unwrap();
            match field {
                "origin" => s.binding.origin = "http://127.0.0.1:1/".into(),
                "owner" => s.binding.owner = Some("@other:example.test".into()),
                "subject" => s.binding.subject = Some("another-subject".into()),
                "client" => s.binding.client_id = "another-client".into(),
                "installation" => s.binding.installation_id = "b".repeat(32),
                "grant" => s.token = "another-grant".into(),
                _ => unreachable!(),
            }
        }
        assert_eq!(
            scope
                .matrix(MatrixOperation::Create(
                    input(Kind::Space),
                    "@owner:example.test".into(),
                    "@_hagency_service:example.test".into()
                ))
                .await
                .err()
                .unwrap()
                .code,
            "matrix_account_changed"
        );
        assert_eq!(
            console
                .0
                .server_login
                .owner_api_inner(
                    Some(&cookie),
                    None,
                    OwnerOperation::AdoptProject {
                        space: "!room:example.test".into()
                    },
                    Some(&scope)
                )
                .await
                .err()
                .unwrap()
                .code,
            "matrix_account_changed"
        );
        console.retire();
    }
}
#[tokio::test]
async fn expected_native_account_and_bounded_closed_inputs_are_required() {
    let (login, server) = fixture(vec![owner_read()]).await;
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let (console, cookie) = console(root.path(), &login).await;
    expect_error(
        invoke(
            &console,
            &cookie,
            Action::List,
            Some(("https://different.invalid/", "@owner:example.test")),
        )
        .await,
        "matrix_account_mismatch",
    );
    done(server).await;
    let mut invalid = input(Kind::Space);
    invalid.name = "x".repeat(129);
    expect_error(
        invoke(&console, &cookie, Action::Start(invalid), None).await,
        "invalid_arguments",
    );
    expect_error(
        invoke(
            &console,
            &cookie,
            Action::Resume {
                id: "valid".into(),
                input: Resume {
                    room_id: Some("!room/escape".into()),
                },
            },
            None,
        )
        .await,
        "invalid_room_id",
    );
    console.retire();
}

#[tokio::test]
async fn http_adapter_and_direct_service_return_identical_complete_recovery() {
    use crate::owner_host::OwnerHost;
    use salvo::test::{ResponseExt, TestClient};
    let (c, proof) = space();
    let (login, server) = fixture(vec![
        owner_read(),
        discovery(),
        (
            "POST /_matrix/client/v3/createRoom",
            json!({"room_id":"!room:example.test"}),
            None,
        ),
        (
            "GET /_matrix/client/v3/rooms/!room:example.test/state",
            proof,
            None,
        ),
        (
            "POST /hagency/api/v1/projects/adopt",
            json!({"project":{"id":"prj_created"}}),
            None,
        ),
        owner_read(),
        owner_read(),
        owner_read(),
        owner_read(),
    ])
    .await;
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let host = OwnerHost::open_native(
        &root.path().join("host"),
        "127.0.0.1:13300".parse().unwrap(),
    )
    .unwrap();
    let console = host.console.clone();
    let cookie = console
        .0
        .authority
        .exchange(&console.owner_access_ticket().unwrap())
        .unwrap();
    let session = login
        .sessions
        .lock()
        .await
        .get(&session_key("cookie"))
        .unwrap()
        .clone();
    console
        .0
        .server_login
        .sessions
        .lock()
        .await
        .insert(session_key(&cookie), session);
    let app = Service::new(host.router());
    let request = |method: &str, path: &str| {
        let url = format!("http://127.0.0.1:13300/console/api/matrix-creations{path}");
        let builder = if method == "POST" {
            TestClient::post(url)
        } else {
            TestClient::get(url)
        };
        builder
            .add_header("host", "127.0.0.1:13300", true)
            .add_header("origin", "http://127.0.0.1:13300", true)
            .add_header("sec-fetch-site", "same-origin", true)
            .add_header("cookie", format!("hagency_console={cookie}"), true)
    };
    let mut response = request("POST", "").json(&c.input).send(&app).await;
    assert_eq!(response.status_code, Some(StatusCode::OK));
    let created = response.take_json::<Value>().await.unwrap();
    assert_eq!(created["creation"]["phase"], "complete");
    assert_eq!(
        value(
            invoke(
                &console,
                &cookie,
                Action::Status(c.input.command_id.clone()),
                None
            )
            .await
            .unwrap()
        ),
        created
    );
    let mut response = request("POST", "/command-1/resume")
        .json(&json!({}))
        .send(&app)
        .await;
    assert_eq!(response.take_json::<Value>().await.unwrap(), created);
    let list = value(invoke(&console, &cookie, Action::List, None).await.unwrap());
    let mut response = request("GET", "").send(&app).await;
    assert_eq!(response.take_json::<Value>().await.unwrap(), list);
    // Boundary rejects query/header/unknown fields before reaching the service.
    let mut response = request("GET", "?arbitrary=1").send(&app).await;
    assert_eq!(response.status_code, Some(StatusCode::FORBIDDEN));
    assert_eq!(
        response.take_json::<Value>().await.unwrap()["code"],
        "invalid_origin"
    );
    let response = request("POST", "")
        .json(&json!({"commandId":"x","kind":"space","name":"x","accessToken":"forbidden"}))
        .send(&app)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::BAD_REQUEST));
    done(server).await;
    console.retire();
}

struct PendingSource {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl crate::console::native::MatrixTokenSource for PendingSource {
    fn access_token(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        crate::console::native::MatrixAccessToken,
                        crate::console::native::NativeError,
                    >,
                > + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(crate::console::native::MatrixAccessToken {
                access_token: "matrix-oauth".into(),
                client_id: "native".into(),
            })
        })
    }
}
#[tokio::test]
async fn sdk_read_cannot_carry_stale_identity_across_await() {
    let (login, server) = fixture(vec![owner_read()]).await;
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let (console, cookie) = console(root.path(), &login).await;
    let scope = Scope::new(&console, &cookie).await.unwrap();
    done(server).await;
    let source = Arc::new(PendingSource {
        entered: Default::default(),
        release: Default::default(),
    });
    console
        .0
        .server_login
        .sessions
        .lock()
        .await
        .get_mut(&session_key(&cookie))
        .unwrap()
        .matrix_source = Some(source.clone());
    let mut request = Box::pin(scope.matrix(MatrixOperation::Create(
        input(Kind::Space),
        "@owner:example.test".into(),
        "@_hagency_service:example.test".into(),
    )));
    tokio::select! {
        _=source.entered.notified()=>{},
        _=&mut request=>panic!("SDK read must be pending"),
    }
    console
        .0
        .server_login
        .sessions
        .lock()
        .await
        .get_mut(&session_key(&cookie))
        .unwrap()
        .binding
        .subject = Some("new-subject".into());
    source.release.notify_one();
    assert_eq!(request.await.err().unwrap().code, "matrix_account_changed");
    console.retire();
}

struct ChangingSource(Arc<std::sync::atomic::AtomicBool>);
impl crate::console::native::MatrixTokenSource for ChangingSource {
    fn access_token(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        crate::console::native::MatrixAccessToken,
                        crate::console::native::NativeError,
                    >,
                > + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            if self.0.load(Ordering::Acquire) {
                return Err(crate::console::native::NativeError {
                    status: 401,
                    code: "matrix_account_changed".into(),
                });
            }
            Ok(crate::console::native::MatrixAccessToken {
                access_token: "matrix-oauth".into(),
                client_id: "native".into(),
            })
        })
    }
}
#[tokio::test]
async fn account_change_after_successful_create_records_uncertainty_and_stops_next_effect() {
    use crate::console::server_login::matrix_creations::tests::fixture_with_hook;
    let changed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (c, proof) = space();
    let (login, server) = fixture_with_hook(
        vec![
            owner_read(),
            discovery(),
            (
                "POST /_matrix/client/v3/createRoom",
                json!({"room_id":"!room:example.test"}),
                None,
            ),
            owner_read(),
            (
                "GET /_matrix/client/v3/rooms/!room:example.test/state",
                proof.clone(),
                None,
            ),
            (
                "GET /_matrix/client/v3/rooms/!room:example.test/state",
                proof,
                None,
            ),
            (
                "POST /hagency/api/v1/projects/adopt",
                json!({"project":{"id":"prj_created"}}),
                None,
            ),
        ],
        {
            let changed = changed.clone();
            move |request| {
                if request == "POST /_matrix/client/v3/createRoom" {
                    changed.store(true, Ordering::Release);
                }
            }
        },
    )
    .await;
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let (console, cookie) = console(root.path(), &login).await;
    console
        .0
        .server_login
        .sessions
        .lock()
        .await
        .get_mut(&session_key(&cookie))
        .unwrap()
        .matrix_source = Some(Arc::new(ChangingSource(changed.clone())));
    expect_error(
        invoke(&console, &cookie, Action::Start(c.input.clone()), None).await,
        "matrix_account_changed",
    );
    let session = login
        .sessions
        .lock()
        .await
        .get(&session_key("cookie"))
        .unwrap()
        .clone();
    let directory = crate::console::owned_agents::journal_profile_directory(
        &root.path().join("state"),
        &session.binding.origin,
        &format!("{}auth/", session.binding.origin),
        "subject",
        &c.owner,
    )
    .unwrap();
    assert!(!directory.join("agent-local.sqlite").exists());
    let saved = load(&directory.join("matrix-creations.json")).unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].phase, "unknown");
    assert_eq!(
        saved[0].last_error.as_deref(),
        Some("matrix_account_changed")
    );
    assert_eq!(saved[0].origin, session.binding.origin);
    assert_eq!(saved[0].owner, c.owner);
    assert_eq!(saved[0].subject, "subject");
    assert!(saved[0].room_id.is_none());
    // Only explicit reauthorization of the original account may recover it.
    changed.store(false, Ordering::Release);
    let result = value(
        invoke(
            &console,
            &cookie,
            Action::Resume {
                id: c.input.command_id,
                input: Resume {
                    room_id: Some("!room:example.test".into()),
                },
            },
            None,
        )
        .await
        .unwrap(),
    );
    assert_eq!(result["creation"]["phase"], "complete");
    done(server).await;
    console.retire();
}

#[tokio::test]
async fn managed_scope_uses_same_sdk_pin_and_synchronous_retirement_gate() {
    use crate::console::server_login::{
        fixture_device, matrix_creations::tests::fixture_with_hook,
    };
    use std::sync::atomic::AtomicBool;
    let live = Arc::new(AtomicBool::new(true));
    let gate = live.clone();
    let (login, server) = fixture_with_hook(vec![discovery()], move |_| {
        gate.store(false, Ordering::Release);
    })
    .await;
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let (console, cookie) = console(root.path(), &login).await;
    console
        .0
        .server_login
        .sessions
        .lock()
        .await
        .get_mut(&session_key(&cookie))
        .unwrap()
        .matrix_source = Some(Arc::new(ChangingSource(Arc::new(
        std::sync::atomic::AtomicBool::new(false),
    ))));

    let origin = login.sessions.lock().await[&session_key("cookie")]
        .binding
        .origin
        .clone();
    let (device, _) = fixture_device(&origin, "subject", "@owner:example.test");
    let gate = live.clone();
    let scope = Scope::management(
        &console,
        &cookie,
        &device,
        Box::new(move || gate.load(Ordering::Acquire)),
    )
    .await
    .unwrap();
    let identity = scope.management_identity(device.device_id());
    assert_eq!(identity.device, "fixture_device");
    assert_eq!(identity.installation, "i".repeat(32));
    assert!(
        scope
            .management_path()
            .ends_with("managed-project-setups-v1.json")
    );
    assert!(
        !scope
            .management_path()
            .parent()
            .unwrap()
            .join("agent-local.sqlite")
            .exists()
    );
    assert_eq!(
        scope
            .matrix(MatrixOperation::Discovery)
            .await
            .err()
            .unwrap()
            .code,
        "management_scope_changed"
    );
    // Retired authority cannot issue a second request, including a cached workflow.
    assert_eq!(
        scope
            .matrix(MatrixOperation::Discovery)
            .await
            .err()
            .unwrap()
            .code,
        "management_scope_changed"
    );
    done(server).await;
    console.retire();
}

#[tokio::test]
async fn managed_scope_refuses_foreign_owner_before_matrix_request() {
    use crate::console::server_login::fixture_device;
    let (login, server) = fixture(vec![]).await;
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let (console, cookie) = console(root.path(), &login).await;
    console
        .0
        .server_login
        .sessions
        .lock()
        .await
        .get_mut(&session_key(&cookie))
        .unwrap()
        .matrix_source = Some(Arc::new(ChangingSource(Arc::new(
        std::sync::atomic::AtomicBool::new(false),
    ))));

    let origin = login.sessions.lock().await[&session_key("cookie")]
        .binding
        .origin
        .clone();
    let (device, _) = fixture_device(&origin, "subject", "@other:example.test");
    assert_eq!(
        Scope::management(&console, &cookie, &device, Box::new(|| true))
            .await
            .err()
            .unwrap()
            .code,
        "matrix_account_changed"
    );
    done(server).await;
    console.retire();
}

#[tokio::test]
async fn pool_agent_room_creation_and_recovery_never_create_a_project() {
    use crate::console::server_login::fixture_device;
    let mut c=command();c.input=input(Kind::AgentRoom);
    let body=MatrixOperation::Create(c.input.clone(),c.owner.clone(),"@_hagency_service:example.test".into()).request().unwrap().2;
    let (login,server)=fixture(vec![discovery(),
        ("POST /_matrix/client/v3/createRoom",json!({"room_id":"!room:example.test"}),body),
        ("GET /_matrix/client/v3/rooms/!room:example.test/state",state(&c),None),
    ]).await;
    let root=hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let (console,cookie)=console(root.path(),&login).await;
    console.0.server_login.sessions.lock().await.get_mut(&session_key(&cookie)).unwrap().matrix_source=Some(Arc::new(ChangingSource(Arc::new(std::sync::atomic::AtomicBool::new(false)))));
    let origin=login.sessions.lock().await[&session_key("cookie")].binding.origin.clone();
    let (device,_)=fixture_device(&origin,"subject","@owner:example.test");
    let created=invoke_pool(&console,&cookie,&device,Box::new(||true),c.input.clone()).await.unwrap();
    assert_eq!(created.creation.phase,Phase::Complete);
    assert_eq!(created.creation.room_id.as_deref(),Some("!room:example.test"));
    assert!(created.creation.project.is_none());assert!(created.creation.project_id.is_none());assert!(created.creation.space_id.is_none());
    done(server).await;
    // The server has closed: exact retries must use the journal without another POST.
    let replay=invoke_pool(&console,&cookie,&device,Box::new(||true),c.input.clone()).await.unwrap();
    assert_eq!(replay,created);
    let mut changed=c.input.clone();changed.name="Different".into();
    assert_eq!(invoke_pool(&console,&cookie,&device,Box::new(||true),changed).await.unwrap_err().code,"creation_command_conflict");
    assert_eq!(invoke_pool(&console,&cookie,&device,Box::new(||false),c.input).await.unwrap_err().code,"management_scope_changed");
    console.retire();
}
