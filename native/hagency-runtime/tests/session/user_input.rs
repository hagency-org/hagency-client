//! Codex 0.166 `item/tool/requestUserInput`: the model's own questions. The
//! session hands a valid question to a host that opted in and accepts exactly
//! one answer for it; otherwise it answers with nothing and the turn goes on.
use super::*;
use hagency_runtime::codex::RequestId;
use hagency_runtime::codex::session::{OTHER_ANSWER_LABEL, UserInputAnswer};
use std::collections::BTreeMap;

fn question_event(id: i64) -> Value {
    json!({"id":id,"method":"item/tool/requestUserInput","params":{
    "threadId":"thread-one","turnId":"turn-one","itemId":"ask-one","isBlocking":true,"autoResolutionMs":null,
    "questions":[
        {"id":"db","header":"Database","question":"Which database should the service use?","isOther":true,"isSecret":false,
         "options":[{"label":"Postgres (Recommended)","description":"Durable and familiar."},{"label":"SQLite","description":"Simplest to run."}]},
        {"id":"token","header":"Token","question":"Paste the deploy token","isOther":false,"isSecret":true,"options":null}
    ]}})
}
fn resolved(id: i64) -> Value {
    note(
        "serverRequest/resolved",
        json!({"threadId":"thread-one","requestId":id}),
    )
}
async fn asked(
    session: &mut Session,
    peer: &mut Peer,
    id: i64,
) -> hagency_runtime::codex::session::UserInputRequest {
    let Update::UserInput(request) = update(session, peer, question_event(id)).await.unwrap()
    else {
        panic!("question handed to the host")
    };
    assert_eq!(session.last_server_request(), Some("tool_user_input"));
    request
}
async fn finish(session: &mut Session, peer: &mut Peer) {
    update(session, peer, item_event("answer", "Done.", false))
        .await
        .unwrap();
    update(session, peer, item_event("answer", "Done.", true))
        .await
        .unwrap();
    assert!(matches!(
        update(session, peer, end("completed")).await.unwrap(),
        Update::TurnEnded
    ));
    assert!(matches!(session.outcome(), Some(Outcome::Completed { text }) if text == "Done."));
}

#[tokio::test]
async fn native_codex_question_round_trip_answers_exactly_once() {
    let (mut session, mut peer) = running().await;
    session.enable_user_input().unwrap();
    assert!(
        session.enable_user_input().is_err(),
        "opting in is once per turn"
    );
    let request = asked(&mut session, &mut peer, 41).await;
    assert_eq!(request.id(), &RequestId::Number(41));
    assert_eq!(request.questions().len(), 2);
    assert_eq!(request.questions()[0].header, "Database");
    assert_eq!(session.pending_user_input(), 1);
    let answers = BTreeMap::from([("db".to_owned(), UserInputAnswer::Option("SQLite".into()))]);
    assert!(
        session
            .respond_user_input(request.answer(&answers).unwrap())
            .await
            .unwrap()
    );
    assert_eq!(
        read(&mut peer.stdin).await,
        json!({"id":41,"result":{"answers":{"db":{"answers":["SQLite"]},"token":{"answers":[]}}}})
    );
    assert_eq!(session.pending_user_input(), 0);
    assert_eq!(
        session.respond_user_input(request.unanswered()).await.err(),
        Some(Error::Scope),
        "a question is answered once"
    );
    // Codex's own resolution of an answered question is ordinary progress.
    assert!(matches!(
        update(&mut session, &mut peer, resolved(41)).await.unwrap(),
        Update::Progress
    ));
    // Another question in the same turn, answered with typed "Other" text.
    let second = asked(&mut session, &mut peer, 42).await;
    let other = BTreeMap::from([("db".to_owned(), UserInputAnswer::Text("MySQL 8".into()))]);
    assert!(
        session
            .respond_user_input(second.answer(&other).unwrap())
            .await
            .unwrap()
    );
    assert_eq!(
        read(&mut peer.stdin).await["result"]["answers"]["db"]["answers"],
        json!([OTHER_ANSWER_LABEL, "user_note: MySQL 8"])
    );
    finish(&mut session, &mut peer).await;
}

#[tokio::test]
async fn native_codex_question_timeout_answers_with_no_answers_and_the_turn_goes_on() {
    let (mut session, mut peer) = running().await;
    session.enable_user_input().unwrap();
    let request = asked(&mut session, &mut peer, 43).await;
    // The host's wait ran out: the model decides on its own.
    assert!(
        session
            .respond_user_input(request.unanswered())
            .await
            .unwrap()
    );
    assert_eq!(
        read(&mut peer.stdin).await,
        json!({"id":43,"result":{"answers":{}}})
    );
    assert!(matches!(
        update(&mut session, &mut peer, resolved(43)).await.unwrap(),
        Update::Progress
    ));
    finish(&mut session, &mut peer).await;
}

#[tokio::test]
async fn native_codex_question_without_a_host_is_answered_with_nothing() {
    // Default sessions, with and without approval control: Codex gets its own
    // "no answers" reply and the turn goes on. The resolution that follows is
    // never mistaken for an approval callback.
    for approvals in [false, true] {
        let (mut session, mut peer) = running().await;
        if approvals {
            session.enable_approvals().unwrap();
        }
        assert!(matches!(
            update(&mut session, &mut peer, question_event(44))
                .await
                .unwrap(),
            Update::Notice
        ));
        assert_eq!(
            read(&mut peer.stdin).await,
            json!({"id":44,"result":{"answers":{}}})
        );
        assert!(matches!(
            update(&mut session, &mut peer, resolved(44)).await.unwrap(),
            Update::Progress
        ));
        assert_eq!(session.phase(), Phase::Running);
        finish(&mut session, &mut peer).await;
    }
    // A host that opted in still never sees a request it could not show.
    let (mut session, mut peer) = running().await;
    session.enable_user_input().unwrap();
    let mut malformed = question_event(45);
    malformed["params"]["questions"] = json!([]);
    assert!(matches!(
        update(&mut session, &mut peer, malformed).await.unwrap(),
        Update::Notice
    ));
    assert_eq!(
        read(&mut peer.stdin).await,
        json!({"id":45,"result":{"answers":{}}})
    );
    finish(&mut session, &mut peer).await;
}

#[tokio::test]
async fn native_codex_question_tool_is_requested_at_thread_start_only_when_offered() {
    // Codex 0.166 lets the model call `request_user_input` in Default mode only
    // with this feature, and fixes a thread's tools when the thread starts.
    for offered in [false, true] {
        let (mut session, mut peer) = fixture(false);
        initialize(&mut session, &mut peer).await;
        if offered {
            session.offer_user_input_tool().unwrap();
        }
        let request = open(&mut session, &mut peer, false).await;
        assert_eq!(request["method"], "thread/start");
        let mut config = json!({
            "sandbox_workspace_write.network_access": false,
            "sandbox_workspace_write.writable_roots": []
        });
        if offered {
            config["features.default_mode_request_user_input"] = true.into();
        }
        assert_eq!(request["params"]["config"], config, "offered: {offered}");
        assert_eq!(
            session.offer_user_input_tool().err(),
            Some(Error::State),
            "too late once the thread started"
        );
        start(&mut session, &mut peer, vec![]).await.unwrap();
        // Offering the tool is not taking the questions: without the turn's
        // own opt-in the session answers at once with no answers.
        assert!(matches!(
            update(&mut session, &mut peer, question_event(46))
                .await
                .unwrap(),
            Update::Notice
        ));
        assert_eq!(
            read(&mut peer.stdin).await,
            json!({"id":46,"result":{"answers":{}}})
        );
        finish(&mut session, &mut peer).await;
    }
}

#[tokio::test]
async fn native_codex_question_secret_is_declined_never_answered() {
    let (mut session, mut peer) = running().await;
    session.enable_user_input().unwrap();
    let request = asked(&mut session, &mut peer, 46).await;
    let secret = &request.questions()[1];
    assert!(secret.is_secret);
    let leaked = BTreeMap::from([("token".to_owned(), UserInputAnswer::Text("hunter2".into()))]);
    assert_eq!(request.answer(&leaked).err(), Some(Error::Policy));
    let answers = BTreeMap::from([(
        "db".to_owned(),
        UserInputAnswer::Option("Postgres (Recommended)".into()),
    )]);
    let response = request.answer(&answers).unwrap();
    assert_eq!(response.result()["answers"]["token"], json!({"answers":[]}));
    assert!(session.respond_user_input(response).await.unwrap());
    let wire = read(&mut peer.stdin).await;
    assert_eq!(wire["result"]["answers"]["token"]["answers"], json!([]));
    assert!(!wire.to_string().contains("hunter2"));
    finish(&mut session, &mut peer).await;
}

#[tokio::test]
async fn native_codex_question_out_of_turn_is_refused() {
    for (key, value) in [
        ("threadId", json!("foreign-thread")),
        ("turnId", json!("foreign-turn")),
        ("turnId", json!(null)),
    ] {
        for enabled in [true, false] {
            let (mut session, mut peer) = running().await;
            if enabled {
                session.enable_user_input().unwrap();
            }
            let mut event = question_event(47);
            event["params"][key] = value.clone();
            assert_eq!(
                update(&mut session, &mut peer, event).await.err(),
                Some(Error::Scope),
                "{key}"
            );
            assert_eq!(session.last_server_request(), Some("tool_user_input"));
            unknown(&session, Error::Scope);
            // Nothing was answered on the substituted identity's behalf: the
            // closed session's input ends without a single byte.
            let mut byte = [0u8; 1];
            assert_eq!(
                timeout(Duration::from_secs(1), peer.stdin.read(&mut byte))
                    .await
                    .unwrap()
                    .unwrap(),
                0,
                "no answer frame"
            );
        }
    }
    let (mut session, mut peer) = running().await;
    session.enable_user_input().unwrap();
    assert_eq!(
        update(&mut session, &mut peer, json!({"id":48,"method":"item/tool/requestUserInput","params":{"private":"not-projected"}}))
            .await
            .err(),
        Some(Error::Scope)
    );
}

#[tokio::test]
async fn native_codex_question_withdrawn_upstream_stops_the_host_wait() {
    let (mut session, mut peer) = running().await;
    session.enable_user_input().unwrap();
    let request = asked(&mut session, &mut peer, 49).await;
    assert!(matches!(
        update(&mut session, &mut peer, resolved(49)).await.unwrap(),
        Update::UserInputResolved {
            id: RequestId::Number(49)
        }
    ));
    assert_eq!(session.pending_user_input(), 0);
    // The host's answer for it is moot: nothing is written and the turn goes on.
    assert!(
        !session
            .respond_user_input(request.unanswered())
            .await
            .unwrap()
    );
    finish(&mut session, &mut peer).await;
    let mut byte = [0u8; 1];
    assert_eq!(
        timeout(Duration::from_secs(1), peer.stdin.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        0,
        "no answer frame for a withdrawn question"
    );
}

#[tokio::test]
async fn native_codex_question_left_open_at_turn_end_is_moot() {
    let (mut session, mut peer) = running().await;
    session.enable_user_input().unwrap();
    let request = asked(&mut session, &mut peer, 50).await;
    // Codex ended the turn without resolving the question: the terminal drain
    // does not turn the moot question into a refused callback.
    finish(&mut session, &mut peer).await;
    // An answer that arrives after the turn is moot, never an error.
    assert!(
        !session
            .respond_user_input(request.unanswered())
            .await
            .unwrap()
    );
    assert_eq!(session.pending_user_input(), 0);
}

#[tokio::test]
async fn native_codex_approval_prompts_are_never_handed_to_the_host() {
    // Codex 0.166 asks an MCP tool approval (and a skill's MCP install) as a
    // question. Whoever answers the room must never grant one: the session
    // declines it with no answers even when the host takes questions.
    for id in [
        "mcp_tool_call_approval_call-1",
        "skill_mcp_dependency_install",
    ] {
        for approvals_enabled in [false, true] {
            // Reject the complete request even when only one of its questions
            // is an approval, or that approval has been marked secret.
            for (mixed, secret) in [(false, false), (true, false), (true, true)] {
                let (mut session, mut peer) = running().await;
                session.enable_user_input().unwrap();
                if approvals_enabled {
                    session.enable_approvals().unwrap();
                }
                let mut event = question_event(51);
                let approval = json!({"id":id,"header":"Approve app tool call?","question":"Allow the deploy tool to run?",
                    "isOther":false,"isSecret":secret,"options":[{"label":"Allow","description":"Run it."},{"label":"Cancel","description":"Do not run it."}]});
                if mixed {
                    event["params"]["questions"]
                        .as_array_mut()
                        .unwrap()
                        .push(approval);
                } else {
                    event["params"]["questions"] = json!([approval]);
                }
                assert!(
                    matches!(
                        update(&mut session, &mut peer, event).await.unwrap(),
                        Update::Notice
                    ),
                    "{id}"
                );
                assert_eq!(
                    read(&mut peer.stdin).await,
                    json!({"id":51,"result":{"answers":{}}}),
                    "{id}"
                );
                assert_eq!(session.pending_user_input(), 0);
                assert!(
                    matches!(
                        update(&mut session, &mut peer, resolved(51)).await.unwrap(),
                        Update::Progress
                    ),
                    "a declined question must not resolve the owner's approval callback"
                );
                // Declining the approval does not disable the room's ordinary
                // questions later in the same turn.
                let normal = asked(&mut session, &mut peer, 52).await;
                let answers =
                    BTreeMap::from([("db".into(), UserInputAnswer::Option("SQLite".into()))]);
                assert!(
                    session
                        .respond_user_input(normal.answer(&answers).unwrap())
                        .await
                        .unwrap()
                );
                assert_eq!(
                    read(&mut peer.stdin).await["result"]["answers"]["db"]["answers"],
                    json!(["SQLite"])
                );
                finish(&mut session, &mut peer).await;
            }
        }
    }
}
