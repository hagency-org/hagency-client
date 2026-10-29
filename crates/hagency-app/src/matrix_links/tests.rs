use super::*;
#[test]
fn parsing_keeps_targets_via_actions_and_consumes_native_failures() {
    let link = parse("https://matrix.to/#/!room:test/$event?via=one.test&via=two.test")
        .unwrap()
        .unwrap();
    assert_eq!(link.room.as_str(), "!room:test");
    assert_eq!(link.event.unwrap().as_str(), "$event");
    assert_eq!(
        link.via
            .iter()
            .map(|server| server.as_str())
            .collect::<Vec<_>>(),
        ["one.test", "two.test"]
    );
    assert!(
        parse("matrix:r/alias:test?action=join")
            .unwrap()
            .unwrap()
            .action
            .is_some()
    );
    assert!(is_user_profile_link("https://matrix.to/#/@alice:test"));
    assert!(!is_user_profile_link(&format!(
        "https://matrix.to/#/@alice:test?{}",
        "x".repeat(MAX_URL_BYTES)
    )));
    assert!(parse("matrix:garbage").unwrap().is_err());
    assert!(parse("https://matrix.to/#/invalid").unwrap().is_err());
    assert!(parse("https://example.org/!room:test").is_none());
    assert!(
        parse(&format!("https://matrix.to?{}", "x".repeat(MAX_URL_BYTES)))
            .unwrap()
            .is_err()
    );
    assert!(
        parse(&format!(
            "https://user@matrix.to/{}",
            "x".repeat(MAX_URL_BYTES)
        ))
        .unwrap()
        .is_err()
    );
    assert!(
        parse(&format!(
            "https://example.org/?{}",
            "x".repeat(MAX_URL_BYTES)
        ))
        .is_none()
    );
}

use crate::sliding_sync::history_tests::{Server, install_link_client};
use makepad_widgets::*;
use std::process::Command;
fn isolated(name: &str) -> bool {
    if std::env::var("HAGENCY_MATRIX_LINK_TEST").as_deref() == Ok(name) {
        return true;
    }
    let profile = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("matrix_links::tests::{name}"),
            "--nocapture",
            "--test-threads=1",
        ])
        .env("HAGENCY_MATRIX_LINK_TEST", name)
        .env("HAGENCY_APP_DATA_DIR", profile.path())
        .env("RINX_DATA_DIR", profile.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "isolated original fixture failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "isolated selector must actually run: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    false
}
fn assert_no_owner_startup() {
    #[cfg(feature = "agent_chat")]
    {
        use crate::hagency::backend::Service;
        assert!(
            Service::existing_current().is_none(),
            "link navigation cannot create an execution owner"
        );
        assert_eq!(
            Service::startup_bootstrap_count(),
            0,
            "native link widgets cannot replay provider startup"
        );
        assert!(!crate::app_data_dir().join("native-owner").exists());
    }
}
fn event(id: &str, thread: Option<&str>) -> serde_json::Value {
    let mut event = serde_json::json!({"type":"m.room.message","event_id":id,
        "room_id":"!room:example.org","sender":"@alice:example.org","origin_server_ts":10,
        "content":{"msgtype":"m.text","body":"Linked original event"}});
    if let Some(root) = thread {
        event["content"]["m.relates_to"] =
            serde_json::json!({"rel_type":"m.thread","event_id":root,"is_falling_back":true});
    }
    event
}
fn fixture() -> Server {
    Server::with_handler(
        false,
        Arc::new(|path| {
            if path.contains("/directory/room/") {
                Some((
                    200,
                    serde_json::json!({"room_id":"!room:example.org","servers":["relay.example.org"]}),
                ))
            } else if path.contains("/event/%24reply") || path.contains("/event/$reply") {
                Some((200, event("$reply", Some("$root"))))
            } else if path.contains("/event/%24root") || path.contains("/event/$root") {
                Some((200, event("$root", None)))
            } else if path.contains("/event/%24ancient") || path.contains("/event/$ancient") {
                Some((200, event("$ancient", None)))
            } else if path.contains("/event/%24wrong") || path.contains("/event/$wrong") {
                Some((200, event("$substitute", None)))
            } else if path.contains("/event/%24denied") || path.contains("/event/$denied") {
                Some((
                    403,
                    serde_json::json!({"errcode":"M_FORBIDDEN","error":"Not allowed"}),
                ))
            } else if path.contains("/context/") {
                let target = if path.contains("reply") {
                    event("$reply", Some("$root"))
                } else {
                    event("$ancient", None)
                };
                Some((
                    200,
                    serde_json::json!({"event":target,"events_before":[],"events_after":[],"state":[],"start":"ctx-start","end":"ctx-end"}),
                ))
            } else if path.contains("/summary") {
                Some((
                    200,
                    serde_json::json!({"room_id":"!new:example.org","name":"New room", "num_joined_members":2,"world_readable":false,"guest_can_join":false,"join_rule":"public"}),
                ))
            } else if path.contains("/join/") {
                Some((200, serde_json::json!({"room_id":"!new:example.org"})))
            } else {
                None
            }
        }),
    )
}
fn controller_for(ticket: &Ticket, phase: Phase, room: Option<ResolvedRoom>) -> Controller {
    Controller {
        pending: Some(Pending {
            ticket: ticket.clone(),
            phase,
            room,
            destination: None,
        }),
        job: None,
    }
}
#[tokio::test]
async fn event_alias_and_actual_context_keep_main_and_thread_identity() {
    if !isolated("event_alias_and_actual_context_keep_main_and_thread_identity") {
        return;
    }
    let server = fixture();
    let room = server.room_for_links().await;
    install_link_client(Some(room.client().clone()));
    let ticket = Ticket::new(&room.client());
    let ready = resolve(
        &room.client(),
        parse("https://matrix.to/#/#alias:example.org/$ancient?via=first.example.org")
            .unwrap()
            .unwrap(),
        ticket.clone(),
    )
    .await
    .unwrap();
    let Outcome::Ready(navigation) = ready else {
        panic!("joined room must not ask to join")
    };
    assert!(matches!(
        navigation.selected,
        SelectedRoom::JoinedRoom { .. }
    ));
    assert_eq!(navigation.event.as_ref().unwrap().as_str(), "$ancient");
    let kind = crate::sliding_sync::TimelineKind::MainRoom {
        room_id: room.room_id().to_owned(),
    };
    let timeline = crate::sliding_sync::build_focused_timeline(&room, &kind, &navigation.event)
        .await
        .unwrap();
    assert!(!timeline.is_threaded());
    let (items, _stream) = timeline.subscribe().await;
    assert!(items.iter().any(|item| {
        item.as_event()
            .is_some_and(|event| event.event_id().is_some_and(|id| id.as_str() == "$ancient"))
    }));
    let reply = conversation(&room, Some("$reply".try_into().unwrap()), ticket.clone())
        .await
        .unwrap();
    assert!(
        matches!(reply.selected, SelectedRoom::Thread {ref thread_root_event_id,..} if thread_root_event_id.as_str()=="$root")
    );
    let thread_kind = crate::sliding_sync::TimelineKind::Thread {
        room_id: room.room_id().to_owned(),
        thread_root_event_id: "$root".try_into().unwrap(),
    };
    let thread = crate::sliding_sync::build_focused_timeline(&room, &thread_kind, &reply.event)
        .await
        .unwrap();
    assert!(thread.is_threaded());
    let (items, _stream) = thread.subscribe().await;
    assert!(items.iter().any(|item| {
        item.as_event()
            .is_some_and(|event| event.event_id().is_some_and(|id| id.as_str() == "$reply"))
    }));
    assert!(
        crate::sliding_sync::build_focused_timeline(&room, &kind, &reply.event)
            .await
            .is_err(),
        "a threaded SDK context must never silently replace the main composer"
    );
    assert!(
        conversation(&room, Some("$denied".try_into().unwrap()), ticket.clone())
            .await
            .is_err()
    );
    assert!(
        conversation(&room, Some("$wrong".try_into().unwrap()), ticket)
            .await
            .is_err()
    );
    let requests = server.requests.lock().unwrap();
    assert!(requests.iter().any(|path| path.contains("/context/")));
    assert!(
        !requests
            .iter()
            .any(|path| path.contains("/messages") || path.contains("/join/")),
        "{requests:?}"
    );
    drop(requests);
    install_link_client(None);
}
#[tokio::test]
async fn confirmation_native_modal_keeps_original_via_and_never_joins_before_click() {
    if !isolated("confirmation_native_modal_keeps_original_via_and_never_joins_before_click") {
        return;
    }
    let server = fixture();
    let existing = server.room_for_links().await;
    install_link_client(Some(existing.client().clone()));
    let ticket = Ticket::new(&existing.client());
    let link = parse(
        "matrix:roomid/new:example.org?via=first.example.org&via=second.example.org&action=join",
    )
    .unwrap()
    .unwrap();
    let outcome = resolve(&existing.client(), link, ticket.clone())
        .await
        .unwrap();
    let Outcome::Confirmation {
        details,
        is_space,
        room,
        ..
    } = outcome
    else {
        panic!("unjoined requires confirmation")
    };
    assert!(
        !server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|path| path.contains("/join/"))
    );
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let widget = cx.with_vm(|vm| {
        makepad_widgets::script_mod(vm);
        crate::app::register_widgets(vm);
        let value = script_eval!(vm,{mod.widgets.JoinLeaveRoomModal {}});
        WidgetRef::script_from_value(vm, value)
    });
    use crate::join_leave_room_modal::JoinLeaveRoomModalWidgetRefExt;
    let stale = Ticket::new(&existing.client());
    stale.cancel();
    widget.as_join_leave_room_modal().set_kind(
        &mut cx,
        crate::join_leave_room_modal::JoinLeaveModalKind::JoinMatrixLink {
            details: (*details).clone(),
            is_space,
            ticket: stale,
        },
        false,
    );
    let uid = widget.button(&cx, ids!(accept_button)).widget_uid();
    let clicks = cx.capture_actions(|cx| {
        cx.widget_action(uid, ButtonAction::Clicked(KeyModifiers::default()))
    });
    let rejected = cx.capture_actions(|cx| {
        widget.handle_event(cx, &Event::Actions(clicks), &mut Scope::empty())
    });
    assert!(!rejected.iter().any(|action| matches!(
        action.downcast_ref::<LinkAction>(),
        Some(LinkAction::Confirm(_))
    )));
    widget.as_join_leave_room_modal().set_kind(
        &mut cx,
        crate::join_leave_room_modal::JoinLeaveModalKind::JoinMatrixLink {
            details: *details,
            is_space,
            ticket: ticket.clone(),
        },
        false,
    );
    let uid = widget.button(&cx, ids!(accept_button)).widget_uid();
    let clicks = cx.capture_actions(|cx| {
        cx.widget_action(uid, ButtonAction::Clicked(KeyModifiers::default()))
    });
    let actions = cx.capture_actions(|cx| {
        widget.handle_event(cx, &Event::Actions(clicks), &mut Scope::empty())
    });
    assert!(actions.iter().any(|action|matches!(action.downcast_ref(),Some(LinkAction::Confirm(current)) if current.same(&ticket))));
    assert!(
        !server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|path| path.contains("/join/"))
    );
    let mut controller = controller_for(&ticket, Phase::Consent, Some(room));
    for action in &actions {
        if let Some(action) = action.downcast_ref::<LinkAction>() {
            controller.action(action);
        }
    }
    tokio::time::timeout(
        Duration::from_secs(5),
        controller
            .job
            .take()
            .expect("native consent must start its original controller job"),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        existing
            .client()
            .get_room(matrix_sdk::ruma::room_id!("!new:example.org"))
            .unwrap()
            .state(),
        RoomState::Joined
    );
    let requests = server.requests.lock().unwrap();
    let joins = requests
        .iter()
        .filter(|path| path.contains("/join/"))
        .collect::<Vec<_>>();
    assert_eq!(joins.len(), 1, "{requests:?}");
    let parsed = url::Url::parse(&format!("http://fixture{}", joins[0])).unwrap();
    let via = parsed
        .query_pairs()
        .filter(|(key, _)| key == "via")
        .map(|(_, value)| value.to_string())
        .collect::<Vec<_>>();
    assert_eq!(via, ["first.example.org", "second.example.org"]);
    drop(requests);
    assert_no_owner_startup();
    install_link_client(None);
}
#[tokio::test]
async fn scope_original_client_nonce_and_dismissal_reject_late_results() {
    if !isolated("scope_original_client_nonce_and_dismissal_reject_late_results") {
        return;
    }
    let server = fixture();
    let room = server.room_for_links().await;
    install_link_client(Some(room.client().clone()));
    let old = Ticket::new(&room.client());
    let mut controller = controller_for(&old, Phase::Resolving, None);
    let navigation = conversation(&room, Some("$ancient".try_into().unwrap()), old.clone())
        .await
        .unwrap();
    controller.cancel();
    assert!(!old.is_current());
    assert!(
        controller
            .action(&LinkAction::Finished(
                old.clone(),
                Ok(Outcome::Ready(navigation))
            ))
            .is_empty()
    );
    let fresh = Ticket::new(&room.client());
    controller = controller_for(
        &fresh,
        Phase::Consent,
        Some(ResolvedRoom {
            room: "!new:example.org".try_into().unwrap(),
            via: vec![],
            event: None,
        }),
    );
    assert!(controller.action(&LinkAction::Confirm(old)).is_empty());
    assert!(
        !server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|path| path.contains("/join/"))
    );
    let effects = controller.action(&LinkAction::Dismiss(fresh.clone()));
    assert!(matches!(effects.as_slice(), [Effect::CloseJoin]));
    assert!(!fresh.is_current());
    // Same user/device but a different SDK BaseClient cannot inherit ownership.
    let replacement = server.room_for_links().await;
    let original = Ticket::new(&room.client());
    install_link_client(Some(replacement.client().clone()));
    assert!(!original.is_current());
    assert!(
        join(
            &room.client(),
            ResolvedRoom {
                room: "!new:example.org".try_into().unwrap(),
                via: vec![],
                event: None
            },
            original
        )
        .await
        .is_err()
    );
    assert!(
        !server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|path| path.contains("/join/"))
    );
    let epoch_ticket = Ticket::new(&replacement.client());
    assert!(epoch_ticket.is_current());
    crate::account_session::close().await;
    assert!(
        !epoch_ticket.is_current(),
        "same client cannot survive an account epoch retirement"
    );
    crate::account_session::open();
    install_link_client(None);
}

#[tokio::test]
async fn event_actual_megolm_inner_relation_rejects_outer_only_thread_and_missing_keys() {
    if !isolated("event_actual_megolm_inner_relation_rejects_outer_only_thread_and_missing_keys") {
        return;
    }
    use matrix_sdk_crypto::{EncryptionSettings, OlmMachine};
    let room_id = matrix_sdk::ruma::room_id!("!room:example.org");
    let source = OlmMachine::new(
        matrix_sdk::ruma::user_id!("@alice:example.org"),
        matrix_sdk::ruma::device_id!("SOURCE"),
    )
    .await;
    source
        .share_room_key(room_id, std::iter::empty(), EncryptionSettings::default())
        .await
        .unwrap();
    let mut events = std::collections::BTreeMap::new();
    for (id, inner_thread, outer_thread) in [
        ("$encrypted-reply", true, true),
        ("$outer-only", false, true),
        ("$encrypted-main", false, false),
    ] {
        let mut content =
            serde_json::json!({"msgtype":"m.text","body":"Authenticated original body"});
        if inner_thread {
            content["m.relates_to"] = serde_json::json!({"rel_type":"m.thread","event_id":"$root","is_falling_back":true});
        }
        let raw = matrix_sdk::ruma::serde::Raw::new(&content)
            .unwrap()
            .cast_unchecked();
        let encrypted = source
            .encrypt_room_event_raw(room_id, "m.room.message", &raw)
            .await
            .unwrap();
        let mut body: serde_json::Value =
            serde_json::from_str(encrypted.content.json().get()).unwrap();
        if outer_thread {
            body["m.relates_to"] =
                serde_json::json!({"rel_type":"m.thread","event_id":"$forged-outer"});
        }
        events.insert(
            id.to_owned(),
            serde_json::json!({"type":"m.room.encrypted","event_id":id,
            "room_id":room_id,"sender":"@alice:example.org","origin_server_ts":50,"content":body}),
        );
    }
    let server = Server::with_handler(
        false,
        Arc::new(move |path| {
            if path.contains("/event/") {
                if path.ends_with("$root") || path.ends_with("%24root") {
                    return Some((200, event("$root", None)));
                }
                for (id, event) in &events {
                    if path.ends_with(id) || path.ends_with(&id.replace('$', "%24")) {
                        return Some((200, event.clone()));
                    }
                }
            }
            None
        }),
    );
    let room = server.room_for_links().await;
    install_link_client(Some(room.client()));
    let ticket = Ticket::new(&room.client());
    assert!(
        conversation(
            &room,
            Some("$encrypted-reply".try_into().unwrap()),
            ticket.clone()
        )
        .await
        .is_err(),
        "unknown keys must not become a main-room navigation"
    );
    let keys = source
        .store()
        .export_room_keys(|session| session.room_id() == room_id)
        .await
        .unwrap();
    assert_eq!(keys.len(), 1);
    let directory = tempfile::tempdir().unwrap();
    let keyfile = directory.path().join("fixture.keys");
    std::fs::write(
        &keyfile,
        matrix_sdk_crypto::encrypt_room_key_export(&keys, "fixture-only", 1).unwrap(),
    )
    .unwrap();
    room.client()
        .encryption()
        .import_room_keys(keyfile, "fixture-only")
        .await
        .unwrap();
    let navigation = conversation(
        &room,
        Some("$encrypted-reply".try_into().unwrap()),
        ticket.clone(),
    )
    .await
    .unwrap();
    assert!(
        matches!(navigation.selected,SelectedRoom::Thread {ref thread_root_event_id,..} if thread_root_event_id.as_str()=="$root"),
        "only authenticated inner root may choose the composer"
    );
    assert!(
        conversation(
            &room,
            Some("$outer-only".try_into().unwrap()),
            ticket.clone()
        )
        .await
        .unwrap_err()
        .contains("could not be authenticated")
    );
    let main = conversation(&room, Some("$encrypted-main".try_into().unwrap()), ticket)
        .await
        .unwrap();
    assert!(matches!(main.selected, SelectedRoom::JoinedRoom { .. }));
    assert!(
        !server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|path| path.contains("/join/")
                || path.contains("/event/%24forged")
                || path.contains("/event/$forged"))
    );
    install_link_client(None);
}

#[tokio::test]
async fn navigation_native_desktop_and_compact_keep_exact_target_and_composer() {
    if !isolated("navigation_native_desktop_and_compact_keep_exact_target_and_composer") {
        return;
    }
    let server = fixture();
    let room = server.room_for_links().await;
    install_link_client(Some(room.client()));
    for target in ["$ancient", "$reply"] {
        let navigation = conversation(
            &room,
            Some(target.try_into().unwrap()),
            Ticket::new(&room.client()),
        )
        .await
        .unwrap();
        crate::home::main_desktop_ui::persistent_navigation_tests::matrix_link_navigation(
            navigation,
        );
        let navigation = conversation(
            &room,
            Some(target.try_into().unwrap()),
            Ticket::new(&room.client()),
        )
        .await
        .unwrap();
        crate::home::home_screen::responsive_layout_tests::matrix_link_navigation(navigation);
    }
    assert_no_owner_startup();
    install_link_client(None);
}

#[tokio::test]
async fn scope_cancelling_an_actual_pending_http_response_cannot_deliver_navigation() {
    if !isolated("scope_cancelling_an_actual_pending_http_response_cannot_deliver_navigation") {
        return;
    }
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let on_request = entered.clone();
    let gated = release.clone();
    let server = Server::with_handler(
        false,
        Arc::new(move |path| {
            if path.contains("/event/") && path.contains("slow") {
                on_request.store(true, Ordering::Release);
                let (lock, wake) = &*gated;
                let (_guard, timeout) = wake
                    .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(5), |released| {
                        !*released
                    })
                    .unwrap();
                assert!(
                    !timeout.timed_out(),
                    "fixture requires explicit response release"
                );
                return Some((200, event("$slow", None)));
            }
            None
        }),
    );
    let room = server.room_for_links().await;
    install_link_client(Some(room.client()));
    let ticket = Ticket::new(&room.client());
    let original = ticket.clone();
    let client = room.client();
    let task = tokio::spawn(async move {
        resolve(
            &client,
            parse("https://matrix.to/#/!room:example.org/$slow")
                .unwrap()
                .unwrap(),
            original,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !entered.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("original event HTTP request actually arrived");
    ticket.cancel();
    {
        let (lock, wake) = &*release;
        *lock.lock().unwrap() = true;
        wake.notify_all();
    }
    assert!(
        task.await.unwrap().is_err(),
        "late original HTTP body must not become a navigation"
    );
    assert!(
        !server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|path| path.contains("/join/"))
    );
    install_link_client(None);
}

#[tokio::test]
async fn scope_queued_input_keeps_its_original_account_before_any_controller_mutation() {
    if !isolated("scope_queued_input_keeps_its_original_account_before_any_controller_mutation") {
        return;
    }
    let server = fixture();
    let original = server.room_for_links().await;
    install_link_client(Some(original.client()));
    let queued = opening(parse("https://matrix.to/#/!room:example.org").unwrap());
    let replacement = server.room_for_links().await;
    install_link_client(Some(replacement.client()));
    let current = Ticket::new(&replacement.client());
    let mut controller = controller_for(&current, Phase::Resolving, None);
    assert!(controller.action(&queued).is_empty());
    assert!(
        controller.matches(&current),
        "stale queued input must not cancel the new account's intent"
    );
    install_link_client(None);
    let before_login = opening(parse("https://matrix.to/#/!room:example.org").unwrap());
    install_link_client(Some(replacement.client()));
    assert!(
        controller.action(&before_login).is_empty(),
        "a previously unauthenticated click cannot acquire a later account"
    );
    assert!(controller.matches(&current));
    controller.cancel();
    install_link_client(None);
}

#[tokio::test]
async fn navigation_direct_contact_view_keeps_thread_ticket_and_drops_retired_intents() {
    if !isolated("navigation_direct_contact_view_keeps_thread_ticket_and_drops_retired_intents") {
        return;
    }
    let server = Server::with_handler(
        false,
        Arc::new(|path| {
            if path.contains("/sync?") {
                return Some((
                    200,
                    serde_json::json!({"next_batch":"s1",
                "account_data":{"events":[{"type":"m.direct","content":{"@bob:example.org":["!room:example.org"]}}]},
                "rooms":{"join":{"!room:example.org":{
                    "state":{"events":[{"type":"m.room.create","state_key":"","event_id":"$create","sender":"@alice:example.org","origin_server_ts":1,"content":{"creator":"@alice:example.org","room_version":"10"}},
                        {"type":"m.room.member","state_key":"@alice:example.org","event_id":"$member","sender":"@alice:example.org","origin_server_ts":2,"content":{"membership":"join"}}]},
                    "timeline":{"events":[event("$recent",None)],"limited":false,"prev_batch":"older"}}}}}),
                ));
            }
            if path.contains("/event/") {
                if path.ends_with("$root") || path.ends_with("%24root") {
                    return Some((200, event("$root", None)));
                }
                return Some((200, event("$reply", Some("$root"))));
            }
            None
        }),
    );
    let room = server.room_for_links().await;
    install_link_client(Some(room.client()));
    assert!(
        !room.direct_targets().is_empty(),
        "real SDK m.direct sync must classify the original room"
    );
    for cancel in [true, false] {
        let navigation = conversation(
            &room,
            Some("$reply".try_into().unwrap()),
            Ticket::new(&room.client()),
        )
        .await
        .unwrap();
        crate::home::mobile::new_chat_tests::matrix_link_navigation(navigation, cancel).await;
    }
    assert_no_owner_startup();
    install_link_client(None);
}

#[tokio::test]
async fn scope_original_error_epoch_survives_queue_and_visible_popup_retirement() {
    if !isolated("scope_original_error_epoch_survives_queue_and_visible_popup_retirement") {
        return;
    }
    let server = fixture();
    let room = server.room_for_links().await;
    install_link_client(Some(room.client()));
    let ticket = Ticket::new(&room.client());
    let mut controller = controller_for(&ticket, Phase::Resolving, None);
    let effects = controller.action(&LinkAction::Finished(
        ticket.clone(),
        Err("original-private-event-marker".into()),
    ));
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let mut popup = cx.with_vm(|vm| {
        makepad_widgets::script_mod(vm);
        crate::app::register_widgets(vm);
        let value = script_eval!(vm,{mod.widgets.RobrixPopupNotificationRightToLeftProgress {}});
        crate::shared::popup_list::RobrixPopupNotification::script_from_value(vm, value)
    });
    let original_epoch = ticket.0.epoch;
    crate::account_session::close().await;
    crate::account_session::open();
    assert_ne!(original_epoch, crate::account_session::epoch());
    for effect in effects {
        let Effect::Error(epoch, message) = effect else {
            panic!("original failure must publish one scoped diagnostic")
        };
        assert_eq!(epoch, original_epoch);
        publish_error(epoch, message);
    }
    popup.handle_event(&mut cx, &Event::Signal, &mut Scope::empty());
    assert!(
        popup.matrix_link_visible_messages(&cx).is_empty(),
        "old diagnostic must not dequeue in a new epoch"
    );
    let current_epoch = crate::account_session::epoch();
    publish_error(current_epoch, "current-private-event-marker".into());
    popup.handle_event(&mut cx, &Event::Signal, &mut Scope::empty());
    assert_eq!(
        popup.matrix_link_visible_messages(&cx),
        ["current-private-event-marker"]
    );
    crate::account_session::close().await;
    crate::account_session::open();
    popup.handle_event(&mut cx, &Event::Signal, &mut Scope::empty());
    assert!(
        popup.matrix_link_visible_messages(&cx).is_empty(),
        "already visible original diagnostics must be removed too"
    );
    install_link_client(None);
}

#[tokio::test]
async fn scope_legacy_membership_modal_retires_original_native_callbacks() {
    if !isolated("scope_legacy_membership_modal_retires_original_native_callbacks") {
        return;
    }
    use crate::join_leave_room_modal::{
        JoinLeaveModalKind, JoinLeaveRoomModalAction, JoinLeaveRoomModalWidgetRefExt,
    };
    let server = fixture();
    let room = server.room_for_links().await;
    install_link_client(Some(room.client()));
    for phase in [Phase::Consent, Phase::Joining] {
        let original = Ticket::new(&room.client());
        let mut controller = controller_for(
            &original,
            phase,
            Some(ResolvedRoom {
                room: "!new:example.org".try_into().unwrap(),
                via: vec![],
                event: None,
            }),
        );
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let widget = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::app::register_widgets(vm);
            let value = script_eval!(vm,{mod.widgets.JoinLeaveRoomModal {}});
            WidgetRef::script_from_value(vm, value)
        });
        widget.as_join_leave_room_modal().set_kind(
            &mut cx,
            JoinLeaveModalKind::JoinMatrixLink {
                details: BasicRoomDetails::RoomId(RoomNameId::empty(
                    "!new:example.org".try_into().unwrap(),
                )),
                is_space: false,
                ticket: original.clone(),
            },
            false,
        );
        let mut callbacks = Vec::new();
        for button in [ids!(accept_button), ids!(cancel_button)] {
            let uid = widget.button(&cx, button).widget_uid();
            let clicks = cx.capture_actions(|cx| {
                cx.widget_action(uid, ButtonAction::Clicked(KeyModifiers::default()))
            });
            callbacks.extend(cx.capture_actions(|cx| {
                widget.handle_event(cx, &Event::Actions(clicks), &mut Scope::empty())
            }));
        }
        assert!(callbacks.iter().any(|a|matches!(a.downcast_ref(),Some(LinkAction::Confirm(ticket)) if ticket.same(&original))));
        assert!(callbacks.iter().any(|a|matches!(a.downcast_ref(),Some(LinkAction::Dismiss(ticket)) if ticket.same(&original))));
        let replacement = JoinLeaveModalKind::JoinRoom {
            details: BasicRoomDetails::RoomId(RoomNameId::empty(
                "!replacement:example.org".try_into().unwrap(),
            )),
            is_space: false,
        };
        let open: Action = Box::new(JoinLeaveRoomModalAction::Open {
            kind: replacement.clone(),
            show_tip: false,
        });
        // The actual App consumes these effects before opening the replacement.
        assert!(matches!(
            controller.observe_navigation(&open).as_slice(),
            [Effect::CloseJoin]
        ));
        widget
            .as_join_leave_room_modal()
            .set_kind(&mut cx, replacement, false);
        let replacement_body = widget.label(&cx, ids!(body)).text();
        assert!(replacement_body.contains("!replacement:example.org"));
        assert!(!original.is_current());
        for callback in callbacks
            .iter()
            .filter_map(|a| a.downcast_ref::<LinkAction>())
        {
            assert!(
                controller.action(callback).is_empty(),
                "late native callback must not close the new modal or join"
            );
        }
        assert!(
            controller
                .action(&LinkAction::Finished(
                    original,
                    Err("late original result".into())
                ))
                .is_empty()
        );
        assert!(controller.pending.is_none());
        assert!(controller.job.is_none());
        assert_eq!(widget.label(&cx, ids!(body)).text(), replacement_body);
    }
    assert!(
        !server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|path| path.contains("/join/"))
    );
    assert_no_owner_startup();
    install_link_client(None);
}
