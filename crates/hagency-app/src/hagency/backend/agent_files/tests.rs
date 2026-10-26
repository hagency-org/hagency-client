use super::*;
use crate::media_transport::tests::Server;
use matrix_sdk::ruma::api::MatrixVersion;
use serde_json::json;
#[tokio::test]
async fn sdk_agent_download_authenticates_bounds_and_verifies_actual_encrypted_bytes_without_cache()
{
    let plaintext = b"private incoming file";
    let mut original = std::io::Cursor::new(plaintext);
    let mut encryptor = matrix_sdk_base::crypto::AttachmentEncryptor::new(&mut original);
    let mut ciphertext = Vec::new();
    encryptor.read_to_end(&mut ciphertext).unwrap();
    let mut encrypted = serde_json::to_value(encryptor.finish()).unwrap();
    encrypted["url"] = json!("mxc://example.org/encrypted");
    let server = Server::new(ciphertext);
    let client = server.client(MatrixVersion::V1_11).await;
    let origin = client.homeserver().to_string();
    let plain=AgentAttachment::from_content(&json!({"msgtype":"m.file","body":"untrusted.bin","url":"mxc://example.org/plain","info":{"size":7}})).unwrap();
    for _ in 0..2 {
        assert_eq!(
            fetch(&client, &origin, "@test:example.org", &plain, || Ok(()))
                .await
                .unwrap(),
            b"preview"
        );
    }
    let requests = server.requests.lock().unwrap().clone();
    let downloads = requests
        .iter()
        .filter(|r| r.contains("/download/example.org/plain"))
        .collect::<Vec<_>>();
    assert_eq!(
        downloads.len(),
        2,
        "each bounded download bypasses shared preview cache"
    );
    assert!(downloads.iter().all(|r| {
        r.to_ascii_lowercase()
            .contains("authorization: bearer fixture-token")
    }));
    let content = json!({"msgtype":"m.file","body":"cipher.bin","file":encrypted,"info":{"size":plaintext.len()}});
    let source = AgentAttachment::from_content(&content).unwrap();
    assert_eq!(
        fetch(&client, &origin, "@test:example.org", &source, || Ok(()))
            .await
            .unwrap(),
        plaintext
    );
    let mut bad = content;
    bad["file"]["hashes"]["sha256"] = json!("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
    let bad = AgentAttachment::from_content(&bad).unwrap();
    assert_eq!(
        fetch(&client, &origin, "@test:example.org", &bad, || Ok(()))
            .await
            .unwrap_err()
            .code,
        "agent_file_integrity_failed"
    );
    for id in ["oversized", "chunked"] {
        let source = AgentAttachment::from_content(
            &json!({"msgtype":"m.file","body":"large.bin","url":format!("mxc://example.org/{id}")}),
        )
        .unwrap();
        assert_eq!(
            fetch(&client, &origin, "@test:example.org", &source, || Ok(()))
                .await
                .unwrap_err()
                .code,
            "agent_file_too_large"
        );
    }
    let old = server.requests.lock().unwrap().len();
    assert!(
        fetch(&client, &origin, "@other:example.org", &plain, || Ok(()))
            .await
            .is_err()
    );
    assert_eq!(server.requests.lock().unwrap().len(), old);
    let legacy = server.client(MatrixVersion::V1_10).await;
    assert!(
        fetch(&legacy, &origin, "@test:example.org", &plain, || Ok(()))
            .await
            .is_err()
    );
    assert_eq!(server.requests.lock().unwrap().len(), old);
    let calls = std::sync::atomic::AtomicUsize::new(0);
    assert!(
        fetch(&client, &origin, "@test:example.org", &plain, || {
            if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 3 {
                Err(source_error("matrix_account_changed"))
            } else {
                Ok(())
            }
        })
        .await
        .is_err()
    );
}
