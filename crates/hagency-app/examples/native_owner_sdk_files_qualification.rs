//! Explicit local Synapse qualification with the actual desktop SDK transport.
//! The local introspection bridge does not qualify production OAuth UI/login.
#[path = "../../hagency/examples/native_owner_files_qualification/qualification.rs"]
mod qualification;
use hagency::native_owner::{self, AgentAttachment, MatrixAccessToken, MatrixTokenSource, NativeError};
use hagency_app::hagency::backend::fetch_agent_attachment_bytes;
use matrix_sdk::Client;
use serde_json::{Value, json};
use std::{io::Read, sync::Arc};
struct Source {
    client: Client,
    origin: String,
}
impl Source {
    fn check(&self) -> Result<(), NativeError> {
        if self
            .client
            .user_id()
            .is_none_or(|u| u.as_str() != "@alice:test")
            || self.client.homeserver().as_str() != self.origin
            || self
                .client
                .device_id()
                .is_none_or(|d| d.as_str() != "JOINT-SDK-ALICE")
        {
            return Err(NativeError {
                status: 401,
                code: "isolated_sdk_account_changed".into(),
            });
        }
        Ok(())
    }
}
impl MatrixTokenSource for Source {
    fn supports_agent_file_receive(&self) -> bool {
        true
    }
    fn access_token(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>> + Send + '_>,
    > {
        Box::pin(async {
            self.check()?;
            let who = self.client.whoami().await.map_err(|_| NativeError {
                status: 401,
                code: "isolated_sdk_whoami_failed".into(),
            })?;
            self.check()?;
            if who.user_id.as_str() != "@alice:test"
                || who.device_id.as_deref() != self.client.device_id()
            {
                return Err(NativeError {
                    status: 401,
                    code: "isolated_sdk_identity_mismatch".into(),
                });
            }
            Ok(MatrixAccessToken {
                access_token: self.client.access_token().ok_or_else(|| NativeError {
                    status: 401,
                    code: "isolated_sdk_login_required".into(),
                })?,
                client_id: "sdk-client".into(),
            })
        })
    }
    fn fetch_agent_attachment(
        &self,
        attachment: AgentAttachment,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<u8>, NativeError>> + Send + '_>,
    > {
        Box::pin(async move {
            self.access_token().await?;
            fetch_agent_attachment_bytes(
                &self.client,
                &self.origin,
                "@alice:test",
                &attachment,
                || self.check(),
            )
            .await
        })
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(result) = native_owner::run_guardian_if_requested() {
        result?;
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let bootstrap: Value =
        serde_json::from_slice(&std::fs::read(std::env::var("HAGENCY_SYNAPSE_BOOTSTRAP")?)?)?;
    let info: Value =
        serde_json::from_slice(&std::fs::read(std::env::var("HAGENCY_FILE_FIXTURE_INFO")?)?)?;
    let origin = info["origin"].as_str().ok_or("origin")?.to_owned();
    let url: url::Url = origin.parse()?;
    let matrix: url::Url = bootstrap["matrixOrigin"]
        .as_str()
        .ok_or("matrix origin")?
        .parse()?;
    if url.host_str() != Some("127.0.0.1")
        || url.scheme() != "http"
        || matrix.host_str() != Some("127.0.0.1")
        || matrix.scheme() != "http"
        || info["realHomeserver"] != true
        || bootstrap["owner"]["user_id"] != "@alice:test"
    {
        return Err("isolated real Synapse fixture required".into());
    }
    let sdk_store = std::path::PathBuf::from(
        std::env::var_os("HAGENCY_FILE_SDK_STORE").ok_or("fresh SDK directory")?,
    );
    if sdk_store.exists() {
        return Err("SDK qualification refuses an existing store".into());
    }
    std::fs::create_dir(&sdk_store)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&sdk_store, std::fs::Permissions::from_mode(0o700))?;
    }
    let client = Client::builder()
        .homeserver_url(&origin)
        .http_client(
            matrix_sdk::reqwest::Client::builder()
                .no_proxy()
                .redirect(matrix_sdk::reqwest::redirect::Policy::none())
                .build()?,
        )
        .sqlite_store(&sdk_store, None)
        .build()
        .await?;
    client
        .matrix_auth()
        .restore_session(
            serde_json::from_value(bootstrap["owner"].clone())?,
            Default::default(),
        )
        .await?;
    let source = Arc::new(Source { client, origin });
    source.access_token().await?;
    if std::env::var("HAGENCY_FILE_SDK_PREFLIGHT").as_deref() == Ok("1") {
        let original: Vec<u8> = (0..131_073).map(|n| n as u8).collect();
        for encrypted in [false, true] {
            let mut file = Value::Null;
            let bytes = if encrypted {
                let mut cursor = std::io::Cursor::new(&original);
                let mut encryptor = matrix_sdk_base::crypto::AttachmentEncryptor::new(&mut cursor);
                let mut bytes = Vec::new();
                encryptor.read_to_end(&mut bytes)?;
                file = serde_json::to_value(encryptor.finish())?;
                bytes
            } else {
                original.clone()
            };
            // Direct local homeserver setup upload does not impersonate a Server
            // send, acquire an execution or touch its original upload witness.
            let response = source
                .client
                .http_client()
                .post(matrix.join("_matrix/media/v3/upload")?)
                .bearer_auth(source.client.access_token().ok_or("SDK token")?)
                .header("content-type", "application/octet-stream")
                .body(bytes)
                .send()
                .await?
                .error_for_status()?;
            let response: Value = serde_json::from_slice(&response.bytes().await?)?;
            let mut content = json!({"msgtype":"m.file","body":"sdk-preflight.bin","info":{"size":original.len()}});
            if encrypted {
                file["url"] = response["content_uri"].clone();
                content["file"] = file;
            } else {
                content["url"] = response["content_uri"].clone();
            }
            let attachment = AgentAttachment::from_content(&content)?;
            if source.fetch_agent_attachment(attachment).await? != original {
                return Err("actual SDK download bytes differ".into());
            }
            if encrypted {
                content["file"]["hashes"]["sha256"] =
                    json!("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
                let invalid = AgentAttachment::from_content(&content)?;
                if source
                    .fetch_agent_attachment(invalid)
                    .await
                    .unwrap_err()
                    .code
                    != "agent_file_integrity_failed"
                {
                    return Err("encrypted integrity refusal required".into());
                }
            }
        }
        println!(
            "Actual Synapse/desktop SDK preflight passed:131073 plain and v2 decrypted bytes; modified hash refused"
        );
        return Ok(());
    }
    qualification::run(source).await
}
