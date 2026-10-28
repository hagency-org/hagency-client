use super::*;

#[test]
fn web_connection_probe_rejects_credentials_authority_fields_and_bad_origins() {
    for address in [
        "https://user:secret@matrix.test",
        "https://matrix.test/?access_token=secret",
        "https://matrix.test/#secret",
        "file:///private",
        "https://matrix.test/private",
        " https://matrix.test",
    ] {
        assert!(
            WebConnectionProbeInput {
                server_name: None,
                url: Some(address.into())
            }
            .validate()
            .is_err()
        );
    }
    assert!(
        serde_json::from_value::<WebConnectionProbeInput>(
            serde_json::json!({"url":"https://matrix.test","owner":"@other:test"})
        )
        .is_err()
    );
    assert!(
        WebConnectionProbeInput {
            server_name: Some("matrix.test:8448".into()),
            url: None
        }
        .validate()
        .is_ok()
    );
    assert!(
        WebConnectionProbeInput {
            server_name: Some("[::1]:8448".into()),
            url: None
        }
        .validate()
        .is_ok()
    );
}

async fn server(body: &'static str, status: u16) -> (String, tokio::task::JoinHandle<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) =
            tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                .await
                .unwrap()
                .unwrap();
        let mut bytes = vec![0; 8192];
        let count = stream.read(&mut bytes).await.unwrap();
        let response = format!(
            "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        String::from_utf8(bytes[..count].to_vec()).unwrap()
    });
    (format!("http://{address}"), task)
}
#[tokio::test]
async fn web_connection_probe_uses_original_route_without_credentials_or_local_server() {
    let (url, request) = server(r#"{"versions":["r0.6.0","v1.1","v1.2","v1.3"]}"#, 200).await;
    let result = probe(WebConnectionProbeInput {
        server_name: None,
        url: Some(url),
    })
    .await
    .unwrap();
    assert!(result.reachable);
    assert_eq!(result.versions, vec!["v1.1", "v1.2", "v1.3"]);
    let sent = request.await.unwrap().to_ascii_lowercase();
    assert!(sent.starts_with("get /_matrix/client/versions "));
    assert!(!sent.contains("authorization:") && !sent.contains("cookie:"));
}
#[tokio::test]
async fn web_connection_probe_distinguishes_http_failure_and_non_matrix_json() {
    for (body, status) in [
        (r#"{"error":"not Matrix"}"#, 200),
        (r#"{"versions":["v1.1"]}"#, 403),
    ] {
        let (url, task) = server(body, status).await;
        let result = probe(WebConnectionProbeInput {
            server_name: None,
            url: Some(url),
        })
        .await
        .unwrap();
        assert!(!result.reachable);
        assert!(result.reason.is_some());
        task.await.unwrap();
    }
}
#[test]
fn web_connection_native_public_edit_never_accepts_secret_or_observed_status() {
    for value in [
        serde_json::json!({"operation":"replace","commandId":"one","id":"matrix.test","expectedRevision":1,"credential":{"asToken":"secret"}}),
        serde_json::json!({"operation":"verify","commandId":"one","id":"matrix.test","expectedRevision":1,"state":"accepted"}),
        serde_json::json!({"operation":"create","commandId":"one","id":"matrix.test","label":"Matrix","apiBaseUrl":"https://matrix.test","credential":"secret"}),
    ] {
        assert!(serde_json::from_value::<WebConnectionEdit>(value).is_err());
    }
}
