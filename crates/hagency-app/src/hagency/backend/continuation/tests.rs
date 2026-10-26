use super::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[tokio::test]
async fn actual_sdk_continuation_uses_original_room_thread_and_transaction_with_account_fences() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
    let observed = requests.clone();
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let requests = observed.clone();
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buffer = [0; 4096];
                let split = loop {
                    let n = stream.read(&mut buffer).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buffer[..n]);
                    if let Some(i) = raw.windows(4).position(|b| b == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let header = String::from_utf8_lossy(&raw[..split]).into_owned();
                let path = header
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_owned();
                let len = header
                    .lines()
                    .find_map(|l| {
                        l.split_once(':')
                            .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while raw.len() < split + len {
                    let n = stream.read(&mut buffer).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buffer[..n]);
                }
                let input = serde_json::from_slice(&raw[split..split + len]).unwrap_or(Value::Null);
                requests.lock().unwrap().push((path.clone(), input));
                let (status, value) = if path.contains("/sync?") || path.ends_with("/sync") {
                    (
                        200,
                        json!({"next_batch":"s1","rooms":{"join":{"!room:test":{"state":{"events":[{"type":"m.room.create","state_key":"","sender":"@owner:test","event_id":"$create:test","origin_server_ts":1,"content":{"creator":"@owner:test","room_version":"10"}},{"type":"m.room.member","state_key":"@owner:test","sender":"@owner:test","event_id":"$member:test","origin_server_ts":2,"content":{"membership":"join"}}]},"timeline":{"events":[],"limited":false,"prev_batch":"s0"},"ephemeral":{"events":[]},"account_data":{"events":[]},"unread_notifications":{}}}},"device_lists":{"changed":[],"left":[]},"device_one_time_keys_count":{}}),
                    )
                } else if path.contains("/send/m.room.message/") {
                    (200, json!({"event_id":"$continuation:test"}))
                } else if path.ends_with("/versions") {
                    (
                        200,
                        json!({"versions":["v1.1","v1.11"],"unstable_features":{}}),
                    )
                } else {
                    (
                        404,
                        json!({"errcode":"M_NOT_FOUND","error":"explicit fixture has no other route"}),
                    )
                };
                let body = serde_json::to_vec(&value).unwrap();
                let headers = format!(
                    "HTTP/1.1 {status} fixture\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(headers.as_bytes()).await;
                let _ = stream.write_all(&body).await;
            });
        }
    });
    let client = matrix_sdk::Client::builder()
        .homeserver_url(&origin)
        .build()
        .await
        .unwrap();
    client
        .restore_session(matrix_sdk::authentication::matrix::MatrixSession {
            meta: matrix_sdk::SessionMeta {
                user_id: matrix_sdk::ruma::user_id!("@owner:test").to_owned(),
                device_id: "OWNER".into(),
            },
            tokens: matrix_sdk::SessionTokens {
                access_token: "local-fixture-only".into(),
                refresh_token: None,
            },
        })
        .await
        .unwrap();
    client
        .sync_once(matrix_sdk::config::SyncSettings::default().timeout(std::time::Duration::ZERO))
        .await
        .unwrap();
    let message = ContinuationMessage {
        owner: "@owner:test".into(),
        room: "!room:test".into(),
        thread: "$original-thread:test".into(),
        agent_mxid: "@agent:test".into(),
        transaction_id: "hagency-continue-original".into(),
        body: "@agent:test\n\nExplicit remaining work".into(),
        require_encrypted: false,
    };
    let id = send(&client, &origin, "@owner:test", message.clone(), || Ok(()))
        .await
        .unwrap();
    assert_eq!(id, "$continuation:test");
    let sent = requests
        .lock()
        .unwrap()
        .iter()
        .filter(|(p, _)| p.contains("/send/"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0]
            .0
            .ends_with("/send/m.room.message/hagency-continue-original")
    );
    assert_eq!(sent[0].1["body"], message.body);
    assert_eq!(sent[0].1["m.mentions"]["user_ids"][0], "@agent:test");
    assert_eq!(
        sent[0].1["m.relates_to"]["event_id"],
        "$original-thread:test"
    );
    let mut private = message.clone();
    private.require_encrypted = true;
    assert!(
        send(&client, &origin, "@owner:test", private, || Ok(()))
            .await
            .is_err()
    );
    let mut foreign = message.clone();
    foreign.owner = "@other:test".into();
    assert!(
        send(&client, &origin, "@owner:test", foreign, || Ok(()))
            .await
            .is_err()
    );
    assert!(
        send(&client, &origin, "@owner:test", message, || Err(
            source_error("matrix_account_changed")
        ))
        .await
        .is_err()
    );
    assert_eq!(
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.contains("/send/"))
            .count(),
        1
    );
    server.abort();
}
