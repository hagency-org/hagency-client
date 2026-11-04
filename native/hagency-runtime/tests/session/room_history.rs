use super::*;

async fn ready() -> (Session, Peer) {
    let (mut session, mut peer) = fixture(true);
    initialize(&mut session, &mut peer).await;
    session.enable_room_history().unwrap();
    let (started, request) = tokio::join!(
        session.start_thread(),
        exchange(&mut peer, thread_result(true), vec![])
    );
    started.unwrap();
    assert_eq!(
        request["params"]["dynamicTools"][0]["name"],
        "read_room_history"
    );
    assert_eq!(request["params"]["sandbox"], "read-only");
    assert_eq!(
        request["params"]["config"]["model_auto_compact_token_limit"],
        24_000
    );
    let (started, _) = tokio::join!(
        session.start_turn("Summarize this room".into()),
        exchange(&mut peer, json!({"turn":turn("inProgress")}), vec![])
    );
    started.unwrap();
    (session, peer)
}
fn request(thread: &str, tool: &str) -> Value {
    json!({"id":91,"method":"item/tool/call","params":{"threadId":thread,"turnId":"turn-one","callId":"history-call","tool":tool,"arguments":{"cursor":null}}})
}

#[tokio::test]
async fn native_room_history_tool_answers_only_original_read_callback_once() {
    let (mut session, mut peer) = ready().await;
    write(&mut peer, request("thread-one", "read_room_history")).await;
    let (update, observation) = session.next_observed_update().await.unwrap();
    assert!(matches!(
        observation.kind(),
        hagency_runtime::codex::session::ObservationKind::Ignored
    ));
    let Update::RoomHistory(call) = update else {
        panic!("history callback missing")
    };
    assert!(call.cursor.is_none());
    assert!(
        session
            .respond_room_history(&call, "x".repeat(13 * 1024))
            .await
            .is_err()
    );
    session
        .respond_room_history(&call, "{\"complete\":true}".into())
        .await
        .unwrap();
    let wire = read(&mut peer.stdin).await;
    assert_eq!(wire["id"], 91);
    assert_eq!(wire["result"]["success"], true);
    assert_eq!(wire["result"]["contentItems"][0]["type"], "inputText");
    assert!(
        session
            .respond_room_history(&call, "duplicate".into())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn native_room_history_tool_rejects_foreign_turn_and_unregistered_tool() {
    for (thread, tool) in [
        ("foreign", "read_room_history"),
        ("thread-one", "send_message"),
    ] {
        let (mut session, mut peer) = ready().await;
        write(&mut peer, request(thread, tool)).await;
        assert!(session.next_update().await.is_err());
    }
}

#[tokio::test]
async fn native_room_history_pagination_survives_compaction_and_more_than_128_items() {
    let (mut session, mut peer) = ready().await;
    for index in 0..150 {
        for (method, status) in [
            ("item/started", "inProgress"),
            ("item/completed", "completed"),
        ] {
            let timestamp = if method == "item/started" {
                "startedAtMs"
            } else {
                "completedAtMs"
            };
            write(&mut peer,note(method,json!({"threadId":"thread-one","turnId":"turn-one",timestamp:10,"item":{"id":format!("history-{index}"),"type":"dynamicToolCall","tool":"read_room_history","arguments":{"cursor":null},"status":status}}))).await;
            assert!(matches!(
                session.next_update().await.unwrap(),
                Update::Item { .. }
            ));
        }
        if index == 75 {
            write(
                &mut peer,
                note(
                    "thread/compacted",
                    json!({"threadId":"thread-one","turnId":"turn-one"}),
                ),
            )
            .await;
            assert!(matches!(
                session.next_update().await.unwrap(),
                Update::Progress
            ));
        }
    }
    assert_eq!(session.phase(), Phase::Running);
}
