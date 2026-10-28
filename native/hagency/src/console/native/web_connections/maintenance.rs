use super::*;
use hagency_store::management::{
    WebConnection, WebConnectionChange, WebConnectionCommand, WebConnectionPage,
    WebConnectionResult, web_connection_credential_digest,
};
use std::time::Instant;

/// Closed, public proposals. Credentials and file content have separate native
/// methods and can never be deserialized through this enum.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum WebConnectionEdit {
    Create {
        #[serde(rename = "commandId")]
        command_id: String,
        id: String,
        label: String,
        #[serde(rename = "apiBaseUrl")]
        api_base_url: String,
    },
    Allowance {
        #[serde(rename = "commandId")]
        command_id: String,
        id: String,
        #[serde(rename = "expectedRevision")]
        expected_revision: u64,
        #[serde(rename = "allocatedTokens")]
        allocated_tokens: Option<u64>,
    },
    Withdraw {
        #[serde(rename = "commandId")]
        command_id: String,
        id: String,
        #[serde(rename = "expectedRevision")]
        expected_revision: u64,
    },
    Issue {
        #[serde(rename = "commandId")]
        command_id: String,
        id: String,
        #[serde(rename = "expectedRevision")]
        expected_revision: u64,
        url: String,
        #[serde(rename = "registrationId")]
        registration_id: Option<String>,
        #[serde(rename = "senderLocalpart")]
        sender_localpart: Option<String>,
        #[serde(rename = "userNamespace")]
        user_namespace: Option<String>,
        exclusive: Option<bool>,
    },
}
impl WebConnectionEdit {
    pub fn validate(&self) -> Result<(), NativeError> {
        command(self.clone()).map(|_| ())
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebConnectionReviewSummary {
    pub command_id: String,
    pub id: String,
    pub operation: String,
    pub expected_revision: u64,
    pub current: Option<WebConnection>,
    pub label: Option<String>,
    pub api_base_url: Option<String>,
    pub allocated_tokens: Option<u64>,
    pub credential_kind: Option<String>,
    pub registration_url: Option<String>,
    pub original_result: Option<WebConnectionResult>,
}
/// Native custody only; the private command is not serializable and cannot be
/// forged from a script-supplied review summary.
pub struct WebConnectionReview {
    pub summary: WebConnectionReviewSummary,
    context: HostContext,
    command: WebConnectionCommand,
}
/// File bytes travel only to the trusted native save dialog.
pub struct NativeConnectionRegistration {
    pub filename: String,
    pub bytes: Vec<u8>,
}
fn origin(value: &str) -> Result<String, NativeError> {
    let url =
        reqwest::Url::parse(value).map_err(|_| failure(400, "invalid_management_arguments"))?;
    validate_origin(&url)?;
    Ok(url.as_str().trim_end_matches('/').to_string())
}
fn command(input: WebConnectionEdit) -> Result<WebConnectionCommand, NativeError> {
    let value = match input {
        WebConnectionEdit::Create {
            command_id,
            id,
            label,
            api_base_url,
        } => WebConnectionCommand {
            command_id,
            id,
            expected_revision: 0,
            change: WebConnectionChange::Create {
                label,
                api_base_url: origin(&api_base_url)?,
            },
        },
        WebConnectionEdit::Allowance {
            command_id,
            id,
            expected_revision,
            allocated_tokens,
        } => WebConnectionCommand {
            command_id,
            id,
            expected_revision,
            change: WebConnectionChange::SetAllowance { allocated_tokens },
        },
        WebConnectionEdit::Withdraw {
            command_id,
            id,
            expected_revision,
        } => WebConnectionCommand {
            command_id,
            id,
            expected_revision,
            change: WebConnectionChange::WithdrawCredential,
        },
        WebConnectionEdit::Issue {
            command_id,
            id,
            expected_revision,
            url,
            registration_id,
            sender_localpart,
            user_namespace,
            exclusive,
        } => {
            let url = registration_url(&url)?;
            WebConnectionCommand {
                command_id,
                id: id.clone(),
                expected_revision,
                change: WebConnectionChange::IssueRegistration {
                    request: hagency_store::IssueSideRegistrationRequest {
                        side: id,
                        url,
                        registration_id,
                        sender_localpart,
                        user_namespace,
                        exclusive,
                    },
                },
            }
        }
    };
    value.validate().map_err(store_error)?;
    Ok(value)
}
fn registration_url(value: &str) -> Result<String, NativeError> {
    let url =
        reqwest::Url::parse(value).map_err(|_| failure(400, "invalid_management_arguments"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || value.len() > 2048
        || value.chars().any(char::is_control)
    {
        return Err(failure(400, "invalid_management_arguments"));
    }
    Ok(url.as_str().trim_end_matches('/').into())
}
fn summarize(
    input: &WebConnectionCommand,
    current: Option<WebConnection>,
) -> WebConnectionReviewSummary {
    let mut summary = WebConnectionReviewSummary {
        command_id: input.command_id.clone(),
        id: input.id.clone(),
        operation: String::new(),
        expected_revision: input.expected_revision,
        current,
        label: None,
        api_base_url: None,
        allocated_tokens: None,
        credential_kind: None,
        registration_url: None,
        original_result: None,
    };
    summary.operation = match &input.change {
        WebConnectionChange::Create {
            label,
            api_base_url,
        } => {
            summary.label = Some(label.clone());
            summary.api_base_url = Some(api_base_url.clone());
            "create"
        }
        WebConnectionChange::SetAllowance { allocated_tokens } => {
            summary.allocated_tokens = *allocated_tokens;
            "allowance"
        }
        WebConnectionChange::WithdrawCredential => "withdraw",
        WebConnectionChange::ReplaceCredential { credential } => {
            summary.credential_kind = credential["kind"].as_str().map(str::to_owned);
            "replace"
        }
        WebConnectionChange::IssueRegistration { request } => {
            summary.registration_url = Some(request.url.clone());
            "issue"
        }
        WebConnectionChange::ImportConfiguration {
            label,
            api_base_url,
            ..
        } => {
            summary.label = Some(label.clone());
            summary.api_base_url = Some(api_base_url.clone());
            summary.credential_kind = Some("appservice".into());
            "import"
        }
        WebConnectionChange::Verification { .. } => "verify",
    }
    .into();
    summary
}
impl NativeManagement {
    pub async fn web_connections(
        &self,
        after: Option<String>,
    ) -> Result<WebConnectionPage, NativeError> {
        let (context, session) = self.authorized().await?;
        let result = session.web_connections(&context, after).await;
        self.finish(&context, result, false).await
    }
    pub async fn web_connection(&self, id: String) -> Result<WebConnection, NativeError> {
        let (context, session) = self.authorized().await?;
        let result = session.web_connection(&context, id).await;
        self.finish(&context, result, false).await
    }
    pub async fn web_connection_result(
        &self,
        command_id: String,
    ) -> Result<Option<WebConnectionResult>, NativeError> {
        let (context, session) = self.authorized().await?;
        let result = session.web_connection_result(&context, command_id).await;
        self.finish(&context, result, false).await
    }
    pub async fn review_web_connection(
        &self,
        input: WebConnectionEdit,
    ) -> Result<WebConnectionReview, NativeError> {
        self.review_connection_command(command(input)?).await
    }
    async fn review_connection_command(
        &self,
        input: WebConnectionCommand,
    ) -> Result<WebConnectionReview, NativeError> {
        input.validate().map_err(store_error)?;
        let (context, session) = self.authorized().await?;
        session
            .web_connection_access(&context, true)
            .map_err(store_error)?;
        let original = session
            .web_connection_result(&context, input.command_id.clone())
            .await
            .map_err(store_error)?;
        if original.as_ref().is_some_and(|r| {
            r.connection.id != input.id
                || r.connection.revision != input.expected_revision.saturating_add(1)
        }) {
            return Err(failure(409, "management_revision_conflict"));
        }
        let current = if input.expected_revision == 0 {
            None
        } else {
            Some(
                session
                    .web_connection(&context, input.id.clone())
                    .await
                    .map_err(store_error)?,
            )
        };
        if original.is_none()
            && current
                .as_ref()
                .is_some_and(|v| v.revision != input.expected_revision)
        {
            return Err(failure(409, "management_revision_conflict"));
        }
        let mut summary = summarize(&input, current);
        summary.original_result = original;
        let review = WebConnectionReview {
            summary,
            context: context.clone(),
            command: input,
        };
        self.finish(&context, Ok(review), false).await
    }
    /// Trusted native file dialog supplies the raw text. No file content or
    /// token is serialized into the review summary or a public pending request.
    pub async fn review_web_connection_import(
        &self,
        command_id: String,
        expected_revision: u64,
        label: String,
        homeserver: String,
        raw: &str,
    ) -> Result<WebConnectionReview, NativeError> {
        let (context, session) = self.authorized().await?;
        session
            .web_connection_access(&context, true)
            .map_err(store_error)?;
        let (registration, appservice, machine, endpoint, generation) =
            crate::bootstrap::palpo_import::parse(raw)
                .map_err(|_| failure(400, "invalid_connection_configuration"))?;
        let credential = serde_json::json!({"kind":"appservice","asToken":appservice["as_token"],"hsToken":appservice["hs_token"],"namespace":appservice["namespace"],"senderLocalpart":appservice["sender_localpart"],"url":appservice["url"]});
        self.review_connection_command(WebConnectionCommand{command_id,id:registration.server_name,expected_revision,change:WebConnectionChange::ImportConfiguration{label,api_base_url:origin(&homeserver)?,credential,transport:serde_json::json!({"mode":"outbound","url":endpoint,"token":machine,"generation":generation})}}).await
    }
    /// The credential object comes from native controls, never Splash args.
    pub async fn review_web_connection_credential(
        &self,
        command_id: String,
        id: String,
        expected_revision: u64,
        credential: serde_json::Value,
    ) -> Result<WebConnectionReview, NativeError> {
        self.review_connection_command(WebConnectionCommand {
            command_id,
            id,
            expected_revision,
            change: WebConnectionChange::ReplaceCredential { credential },
        })
        .await
    }
    pub async fn apply_web_connection(
        &self,
        review: WebConnectionReview,
        confirmed_until: Instant,
    ) -> Result<WebConnectionResult, NativeError> {
        if Instant::now() >= confirmed_until {
            return Err(failure(403, "provider_confirmation_expired"));
        }
        let (context, session) = self.authorized().await?;
        if context != review.context {
            return Err(failure(401, "management_scope_changed"));
        }
        session
            .web_connection_access(&context, true)
            .map_err(store_error)?;
        // The exact original command goes through the writer's digest check;
        // a reused ID cannot turn an unrelated old receipt into this result.
        let result = session.apply_web_connection(&context, review.command).await;
        self.finish(&context, result, true).await
    }
    pub async fn download_web_connection_registration(
        &self,
        id: String,
        expected_revision: u64,
    ) -> Result<NativeConnectionRegistration, NativeError> {
        let (context, session) = self.authorized().await?;
        let snapshot = session
            .web_connection_private(&context, id)
            .await
            .map_err(store_error)?;
        if snapshot.connection.revision != expected_revision {
            return Err(failure(409, "management_revision_conflict"));
        }
        let bytes = snapshot
            .registration
            .ok_or_else(|| failure(404, "connection_registration_unavailable"))?
            .into_bytes();
        let filename = format!(
            "{}.yaml",
            snapshot
                .connection
                .id
                .chars()
                .take(100)
                .map(|c| if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                })
                .collect::<String>()
        );
        self.finish(
            &context,
            Ok(NativeConnectionRegistration { filename, bytes }),
            false,
        )
        .await
    }
    pub async fn verify_web_connection(
        &self,
        command_id: String,
        id: String,
        expected_revision: u64,
    ) -> Result<WebConnectionResult, NativeError> {
        let (context, session) = self.authorized().await?;
        session
            .web_connection_access(&context, true)
            .map_err(store_error)?;
        if let Some(original) = session
            .web_connection_result(&context, command_id.clone())
            .await
            .map_err(store_error)?
        {
            if original.operation != "verify"
                || original.connection.id != id
                || original.connection.revision != expected_revision.saturating_add(1)
            {
                return Err(failure(409, "management_revision_conflict"));
            }
            return self.finish(&context, Ok(original), false).await;
        }
        let snapshot = session
            .web_connection_private(&context, id.clone())
            .await
            .map_err(store_error)?;
        if snapshot.connection.revision != expected_revision {
            return Err(failure(409, "management_revision_conflict"));
        }
        let mut staged = snapshot.pending.is_some();
        let mut credential = snapshot
            .pending
            .as_ref()
            .or(snapshot.credential.as_ref())
            .ok_or_else(|| failure(409, "connection_credential_required"))?;
        let mut state = whoami(&snapshot.connection, credential).await?;
        if staged && state != "accepted" {
            if let Some(live) = snapshot.credential.as_ref() {
                session
                    .web_connection_access(&context, true)
                    .map_err(store_error)?;
                let (current, _) = self.authorized().await?;
                if current != context {
                    return Err(failure(401, "management_scope_changed"));
                }
                credential = live;
                staged = false;
                state = whoami(&snapshot.connection, live).await?;
            }
        }
        session
            .web_connection_access(&context, true)
            .map_err(store_error)?;
        let (current, _) = self.authorized().await?;
        if current != context {
            return Err(failure(401, "management_scope_changed"));
        }
        let input = WebConnectionCommand {
            command_id,
            id,
            expected_revision,
            change: WebConnectionChange::Verification {
                state,
                credential_digest: web_connection_credential_digest(credential)
                    .map_err(store_error)?,
                staged,
            },
        };
        let result = session.apply_web_connection(&context, input).await;
        self.finish(&context, result, true).await
    }
}
async fn whoami(
    connection: &WebConnection,
    credential: &hagency_store::Credential,
) -> Result<String, NativeError> {
    // Preserve the old whoami classification while requiring an exact observed
    // representative identity, never merely any nonempty returned user ID.
    let side = hagency_store::SideRecord {
        id: connection.id.clone(),
        label: connection.label.clone(),
        server_name: connection.id.clone(),
        api_base_url: Some(origin(&connection.api_base_url)?),
        credential_kind: Some(credential.kind.clone()),
        has_credential: true,
        awaiting_install: false,
        awaiting_install_since: None,
        sender_localpart: credential.sender_localpart.clone(),
        appservice_url: credential.url.clone(),
        namespace: credential.namespace.clone(),
        access_issued_at: None,
        access_state: "unverified".into(),
        access_detail: None,
        access_checked_at: None,
        allocated_tokens: connection.allocated_tokens,
        representative: None,
        projects: vec![],
        active: true,
        created_at: 0,
        updated_at: 0,
    };
    let observed =
        crate::console::side_lifecycle::whoami(&connection.api_base_url, &side, credential).await;
    if observed.state == "accepted" {
        let Some(mxid) = observed.mxid else {
            return Ok("rejected".into());
        };
        let expected = credential
            .sender_localpart
            .as_ref()
            .map(|s| format!("@{s}:{}", connection.id));
        if expected.as_ref().is_some_and(|v| v != &mxid)
            || !mxid.ends_with(&format!(":{}", connection.id))
        {
            return Ok("rejected".into());
        }
    }
    Ok(observed.state)
}
