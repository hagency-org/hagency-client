//! Exercise the production invitation helper with the real SDK crypto/history
//! preflight. No model calls, external server or paid account is used.
use matrix_sdk::{
    config::RequestConfig,
    ruma::{device_id, user_id},
    test_utils::mocks::MatrixMockServer,
};
use serde_json::{Value, json};
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{method, path_regex, query_param},
};

async fn encrypted_room(server: &MatrixMockServer) -> matrix_sdk::Room {
    server.mock_crypto_endpoints_preset().await;
    server
        .mock_add_room_keys_version()
        .ok()
        .mock_once()
        .mount()
        .await;
    let client = server
        .client_builder_for_crypto_end_to_end(user_id!("@inviter:localhost"), device_id!("INVITER"))
        .on_builder(|builder| {
            builder
                .with_enable_share_history_on_invite(true)
                .request_config(RequestConfig::new().retry_limit(0))
        })
        .build()
        .await;
    client
        .encryption()
        .bootstrap_cross_signing(None)
        .await
        .unwrap();
    client
        .encryption()
        .request_user_identity(client.user_id().unwrap())
        .await
        .unwrap();
    client.encryption().backups().create().await.unwrap();
    Mock::given(method("POST"))
        .and(path_regex(r"/_matrix/client/v3/createRoom"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"room_id":"!fresh-encrypted:localhost"})),
        )
        .expect(1)
        .mount(server.server())
        .await;
    let room = client
        .create_room(matrix_sdk::ruma::api::client::room::create_room::v3::Request::new())
        .await
        .unwrap();
    Mock::given(method("GET"))
        .and(path_regex(
            r"/_matrix/client/v3/rooms/.*/state/m.room.encryption/",
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"algorithm":"m.megolm.v1.aes-sha2"})),
        )
        .mount(server.server())
        .await;
    room
}

async fn invite_with_backup_response(status: u16, body: Value, should_invite: bool) {
    let server = MatrixMockServer::new().await;
    let room = encrypted_room(&server).await;
    Mock::given(method("GET"))
        .and(path_regex(r"/_matrix/client/v3/room_keys/keys/.*"))
        .and(query_param("version", "1"))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .expect(1)
        .mount(server.server())
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/_matrix/client/v3/rooms/.*/invite"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(u64::from(should_invite))
        .mount(server.server())
        .await;
    // There is no history to upload, and no message may be sent to make an
    // invitation work. Errors must not fall back to a direct plaintext invite.
    Mock::given(method("POST"))
        .and(path_regex(r".*/upload.*"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(server.server())
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(r".*/send/.*"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(server.server())
        .await;
    let result = super::invite_room_member(&room, user_id!("@guest:localhost")).await;
    assert_eq!(result.is_ok(), should_invite, "{result:?}");
    assert!(!matches!(
        room.latest_encryption_state().await.unwrap(),
        matrix_sdk::EncryptionState::NotEncrypted
    ));
    server.server().verify().await;
}

#[tokio::test]
async fn encrypted_invitation_accepts_an_empty_valid_backup_without_sending_a_message() {
    invite_with_backup_response(200, json!({"sessions":{}}), true).await;
}

#[tokio::test]
async fn encrypted_invitation_does_not_mask_a_missing_backup_version() {
    invite_with_backup_response(
        404,
        json!({"errcode":"M_NOT_FOUND","error":"Unknown backup version"}),
        false,
    )
    .await;
}

#[tokio::test]
async fn encrypted_invitation_does_not_mask_permission_or_server_errors() {
    invite_with_backup_response(
        403,
        json!({"errcode":"M_FORBIDDEN","error":"Denied"}),
        false,
    )
    .await;
    invite_with_backup_response(
        500,
        json!({"errcode":"M_UNKNOWN","error":"Unavailable"}),
        false,
    )
    .await;
}

#[tokio::test]
async fn encrypted_invitation_rejects_a_malformed_backup_response() {
    invite_with_backup_response(200, json!({"sessions":[]}), false).await;
}
