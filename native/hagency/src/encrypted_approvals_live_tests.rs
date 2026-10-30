//! Actual SDK transport against a disposable localhost Matrix homeserver.
//! No provider call, management budget, existing account, or shared room.
use super::*;
use matrix_sdk::{
    SessionMeta, SessionTokens, authentication::matrix::MatrixSession, config::SyncSettings,
    store::RoomLoadSettings,
};

async fn register(origin: &str, name: &str) -> Value {
    let response = reqwest::Client::new()
        .post(format!("{origin}/_matrix/client/v3/register"))
        .json(&json!({"username":format!("{name}_{}",nonce().unwrap()),
            "password":nonce().unwrap(),"auth":{"type":"m.login.dummy"},
            "device_id":format!("HAGE2EE{}",nonce().unwrap())}))
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "disposable Matrix registration: {}",
        response.status()
    );
    response.json().await.unwrap()
}
async fn client(origin: &str, session: &Value) -> Client {
    let client = Client::builder()
        .homeserver_url(origin)
        .build()
        .await
        .unwrap();
    client
        .matrix_auth()
        .restore_session(
            MatrixSession {
                meta: SessionMeta {
                    user_id: session["user_id"].as_str().unwrap().try_into().unwrap(),
                    device_id: session["device_id"].as_str().unwrap().into(),
                },
                tokens: SessionTokens {
                    access_token: session["access_token"].as_str().unwrap().into(),
                    refresh_token: None,
                },
            },
            RoomLoadSettings::default(),
        )
        .await
        .unwrap();
    client
        .sync_once(SyncSettings::default().timeout(Duration::ZERO))
        .await
        .unwrap();
    client
}
async fn sync(client: &Client) {
    client
        .sync_once(SyncSettings::default().timeout(Duration::ZERO))
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires disposable localhost Matrix with dummy registration: HAGENCY_TEST_MATRIX_URL"]
async fn encrypted_approval_actual_sdk_without_room_or_manager_keys() {
    let origin = std::env::var("HAGENCY_TEST_MATRIX_URL").expect("explicit disposable homeserver");
    let url = reqwest::Url::parse(&origin).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert_eq!(url.scheme(), "http");
    // Some homeservers put their first registered administrator into a room.
    let _bootstrap = register(&origin, "bootstrap").await;
    let agent_session = register(&origin, "agent").await;
    let owner_session = register(&origin, "owner").await;
    let manager_session = register(&origin, "web_manager").await;
    let agent = client(&origin, &agent_session).await;
    let owner = client(&origin, &owner_session).await;
    let agent_inbox = Inbox::attach(&agent);
    let owner_inbox = Inbox::attach(&owner);
    assert!(agent.joined_rooms().is_empty());
    assert!(owner.joined_rooms().is_empty());
    let secret = format!("private-command-{}", nonce().unwrap());
    let request = json!({"id":7,"method":"item/commandExecution/requestApproval",
        "params":{"command":secret,"cwd":"/workspace"},"sessionAllowed":true,"managerCanDecide":false});
    let salt = nonce().unwrap();
    let approval = json!({"id":format!("sap_{}",nonce().unwrap()),"workId":"isolated_work",
        "agentMxid":agent_session["user_id"],"ownerMxid":owner_session["user_id"],
        "managerMxid":manager_session["user_id"],"expiresAtMs":now()+120_000,
        "request":metadata(&request,&salt,agent.device_id().unwrap().as_str()).unwrap()});
    let row = json!({"digest":hash(&approval).unwrap(),"approval":approval});
    assert!(!row.to_string().contains(&secret));
    tokio::time::timeout(
        Duration::from_secs(30),
        send_request(&agent, &row, &request, &salt),
    )
    .await
    .unwrap()
    .unwrap();
    // Inspect the actual homeserver response before handing it to the SDK.
    let wire: Value = reqwest::Client::new()
        .get(format!("{origin}/_matrix/client/v3/sync"))
        .bearer_auth(owner_session["access_token"].as_str().unwrap())
        .query(&[("timeout", "0")])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let events = wire["to_device"]["events"].as_array().unwrap();
    assert!(!events.is_empty());
    assert!(events.iter().all(|e| e["type"] == "m.room.encrypted"));
    assert!(!wire.to_string().contains(&secret));
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            sync(&owner).await;
            if let Some(details) = owner_inbox.details(&row).unwrap() {
                assert_eq!(details, request);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    send_decision(&owner, &row, "approve_session")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            sync(&agent).await;
            if agent_inbox.has_decision(&row).unwrap() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        agent_inbox
            .matches_decision(&row, owner.user_id().unwrap().as_str(), "approve_session")
            .unwrap()
    );
    assert!(
        !agent_inbox
            .matches_decision(&row, owner.user_id().unwrap().as_str(), "approve_once")
            .unwrap()
    );
    assert!(
        !agent_inbox
            .matches_decision(
                &row,
                manager_session["user_id"].as_str().unwrap(),
                "approve_session"
            )
            .unwrap()
    );
    assert!(owner.joined_rooms().is_empty());
    // The manager and the executor share a Megolm room; the owner still need
    // not join it. Check the actual room key delivery, prompt and reply too.
    let manager = client(&origin, &manager_session).await;
    let room: Value = reqwest::Client::new()
        .post(format!("{origin}/_matrix/client/v3/createRoom"))
        .bearer_auth(manager_session["access_token"].as_str().unwrap())
        .json(
            &json!({"preset":"private_chat","invite":[agent.user_id().unwrap()],
            "initial_state":[{"type":"m.room.encryption","state_key":"",
                "content":{"algorithm":"m.megolm.v1.aes-sha2"}}]}),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let room_id: matrix_sdk::ruma::OwnedRoomId =
        room["room_id"].as_str().unwrap().try_into().unwrap();
    agent.join_room_by_id(&room_id).await.unwrap();
    sync(&agent).await;
    sync(&manager).await;
    let manager_room = manager.get_room(&room_id).unwrap();
    let agent_room = agent.get_room(&room_id).unwrap();
    let sent = manager_room
        .send(matrix_sdk::ruma::events::room::message::RoomMessageEventContent::text_plain(&secret))
        .await
        .unwrap();
    assert_room_message(
        &origin,
        &agent_session,
        &agent,
        &room_id,
        &sent.response.event_id,
        &secret,
    )
    .await;
    let answer = format!("private-answer-{}", nonce().unwrap());
    let sent = agent_room
        .send(matrix_sdk::ruma::events::room::message::RoomMessageEventContent::text_plain(&answer))
        .await
        .unwrap();
    assert_room_message(
        &origin,
        &manager_session,
        &manager,
        &room_id,
        &sent.response.event_id,
        &answer,
    )
    .await;
    sync(&owner).await;
    assert!(owner.joined_rooms().is_empty());
    manager.matrix_auth().logout().await.unwrap();
    owner.matrix_auth().logout().await.unwrap();
    agent.matrix_auth().logout().await.unwrap();
}

async fn assert_room_message(
    origin: &str,
    session: &Value,
    receiver: &Client,
    room: &matrix_sdk::ruma::RoomId,
    event: &matrix_sdk::ruma::EventId,
    body: &str,
) {
    let wire: Value = reqwest::Client::new()
        .get(format!(
            "{origin}/_matrix/client/v3/rooms/{room}/event/{event}"
        ))
        .bearer_auth(session["access_token"].as_str().unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(wire["type"], "m.room.encrypted");
    assert!(!wire.to_string().contains(body));
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            sync(receiver).await;
            let event = receiver
                .get_room(room)
                .unwrap()
                .event(event, None)
                .await
                .unwrap();
            if let matrix_sdk::deserialized_responses::TimelineEventKind::Decrypted(decrypted) =
                event.kind
            {
                let value: Value = decrypted.event.deserialize_as().unwrap();
                assert_eq!(value["content"]["body"], body);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
}
