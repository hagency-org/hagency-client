//! Explicit paid public NativeOwner qualification with loopback service doubles.
#[path = "native_owner_files_qualification/qualification.rs"]
mod qualification;
use hagency::native_owner::{self, MatrixAccessToken, MatrixTokenSource, NativeError};
use std::sync::Arc;
struct Source {
    origin: String,
}
impl MatrixTokenSource for Source {
    fn access_token(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<MatrixAccessToken, NativeError>> + Send + '_>,
    > {
        Box::pin(async {
            Ok(MatrixAccessToken {
                access_token: "fixture-sdk-only".into(),
                client_id: "sdk-client".into(),
            })
        })
    }
    fn supports_agent_file_receive(&self) -> bool {
        true
    }
    fn fetch_agent_attachment(
        &self,
        attachment: native_owner::AgentAttachment,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<u8>, NativeError>> + Send + '_>,
    > {
        Box::pin(async move {
            attachment.validate().map_err(|_| NativeError {
                status: 400,
                code: "fixture_original_invalid".into(),
            })?;
            if attachment.source["url"] != "mxc://test/joint-incoming" {
                return Err(NativeError {
                    status: 403,
                    code: "fixture_original_required".into(),
                });
            }
            let result = async {
                let response = reqwest::Client::new()
                    .get(format!(
                        "{}_matrix/client/v1/media/download/test/joint-incoming",
                        self.origin
                    ))
                    .bearer_auth("fixture-sdk-only")
                    .send()
                    .await?
                    .error_for_status()?;
                response.bytes().await
            }
            .await;
            result.map(|b| b.to_vec()).map_err(|_| NativeError {
                status: 503,
                code: "fixture_media_unavailable".into(),
            })
        })
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The same executable must implement guardian mode before entering any
    // owner/client initialization. A libtest binary cannot provide this entry.
    if let Some(result) = native_owner::run_guardian_if_requested() {
        result?;
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let info: serde_json::Value = serde_json::from_slice(&std::fs::read(std::env::var(
                "HAGENCY_FILE_FIXTURE_INFO",
            )?)?)?;
            let origin = info["origin"].as_str().ok_or("fixture origin")?.to_owned();
            qualification::run(Arc::new(Source { origin })).await
        })
}
