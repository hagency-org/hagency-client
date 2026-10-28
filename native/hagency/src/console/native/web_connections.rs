//! The web client's external Matrix discovery check, without a local web server.
//!
//! Reuses the original diagnostic handler. This is an unauthenticated Matrix
//! versions/discovery request, not connection admission or credential approval.
//! The owner-only host intentionally has no legacy fleet database or transport;
//! registration/credential writes must not be routed to that absent backend.
use super::{NativeError, NativeManagement, failure, management::store_error};
use hagency_store::management::{HostContext, Permission, Session};
use salvo::{
    Service,
    http::StatusCode,
    test::{ResponseExt, TestClient},
};
use serde::{Deserialize, Serialize};

#[path = "web_connections/maintenance.rs"]
mod maintenance;
pub use maintenance::{
    NativeConnectionRegistration, WebConnectionEdit, WebConnectionReview,
    WebConnectionReviewSummary,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebConnectionProbeInput {
    pub server_name: Option<String>,
    pub url: Option<String>,
}

impl WebConnectionProbeInput {
    pub fn validate(&self) -> Result<(), NativeError> {
        let name = self.server_name.as_deref().unwrap_or("");
        let address = self.url.as_deref().unwrap_or("");
        if (name.is_empty() && address.is_empty())
            || name.len() > 255
            || address.len() > 2048
            || [name, address]
                .iter()
                .any(|s| s.trim() != *s || s.chars().any(char::is_control))
        {
            return Err(failure(400, "invalid_management_arguments"));
        }
        if !name.is_empty() {
            // A server identifier is distinct from a URL. In particular,
            // username/password or query text must not enter discovery prose.
            if name.chars().any(char::is_whitespace) || name.contains(['/', '?', '#', '@']) {
                return Err(failure(400, "invalid_management_arguments"));
            }
            let parsed = reqwest::Url::parse(&format!("https://{name}"))
                .map_err(|_| failure(400, "invalid_management_arguments"))?;
            validate_origin(&parsed)?;
        }
        if !address.is_empty() {
            let parsed = reqwest::Url::parse(address)
                .map_err(|_| failure(400, "invalid_management_arguments"))?;
            validate_origin(&parsed)?;
        }
        Ok(())
    }
}

fn validate_origin(url: &reqwest::Url) -> Result<(), NativeError> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(failure(400, "invalid_management_arguments"));
    }
    Ok(())
}

/// Public projection: no credential, file path, or imported registration.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebConnectionProbe {
    pub origin: String,
    pub via: String,
    pub reachable: bool,
    pub status: Option<u16>,
    pub versions: Vec<String>,
    pub reason: Option<String>,
}

fn authorize_probe(session: &Session, context: &HostContext) -> Result<(), NativeError> {
    session
        .authorize(context, Permission::ReadConnections)
        .map_err(store_error)
}

impl NativeManagement {
    pub async fn probe_web_connection(
        &self,
        input: WebConnectionProbeInput,
    ) -> Result<WebConnectionProbe, NativeError> {
        let (context, session) = self.authorized().await?;
        authorize_probe(&session, &context)?;
        input.validate()?;
        let result = probe(input).await;
        // Both the original package lease and current Matrix profile must still
        // be valid when the network observation is returned to the mini-app.
        authorize_probe(&session, &context)?;
        let (current, latest) = self.authorized().await?;
        if current != context {
            return Err(failure(401, "management_scope_changed"));
        }
        authorize_probe(&latest, &current)?;
        result
    }
}

async fn probe(input: WebConnectionProbeInput) -> Result<WebConnectionProbe, NativeError> {
    input.validate()?;
    // In-process Salvo dispatch reuses matrix_diag's exact discovery, timeout,
    // response bound and no-redirect/no-credential HTTP client. No socket is
    // opened by this adapter and no console/App/domain is constructed.
    let service = Service::new(crate::console::matrix_diag::router());
    let mut response = TestClient::post("http://hagency-native.invalid/matrix/probe")
        .json(&serde_json::json!({"server_name": input.server_name, "url": input.url}))
        .send(&service)
        .await;
    if !response.status_code.unwrap_or(StatusCode::OK).is_success() {
        return Err(failure(400, "invalid_management_arguments"));
    }
    let value: serde_json::Value = response
        .take_json()
        .await
        .map_err(|_| failure(503, "management_invalid_result"))?;
    let required = |v: &serde_json::Value| {
        v.as_str()
            .map(str::to_owned)
            .ok_or_else(|| failure(503, "management_invalid_result"))
    };
    let result = &value["probe"];
    let versions = match result.get("versions") {
        None => Vec::new(),
        Some(serde_json::Value::Array(values)) if values.len() <= 3 => values
            .iter()
            .map(|v| {
                required(v).and_then(|s| {
                    if s.len() <= 128 {
                        Ok(s)
                    } else {
                        Err(failure(503, "management_invalid_result"))
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(failure(503, "management_invalid_result")),
    };
    Ok(WebConnectionProbe {
        origin: required(&value["origin"])?,
        via: required(&value["via"])?,
        reachable: result["reachable"]
            .as_bool()
            .ok_or_else(|| failure(503, "management_invalid_result"))?,
        status: result["status"]
            .as_u64()
            .and_then(|v| u16::try_from(v).ok()),
        versions,
        reason: result["reason"].as_str().map(str::to_owned),
    })
}

#[cfg(test)]
#[path = "web_connections/tests.rs"]
mod tests;
