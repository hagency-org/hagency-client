use super::*;
use crate::console::server_login::OwnerError;
use serde_json::{Value, json};
pub(in crate::console::native) struct ServerOperation(AgentOperation);
pub(super) enum AgentOperation {
    Poll {
        after: Option<String>,
    },
    Recover {
        id: String,
        digest: String,
    },
    Result {
        id: String,
        digest: String,
        token: String,
        receipt: Box<Receipt>,
    },
}
impl ServerOperation {
    pub(super) fn new(op: AgentOperation) -> Result<Self, NativeError> {
        match &op {
            AgentOperation::Poll { after } => {
                if let Some(id) = after {
                    super::super::key(id)?;
                }
            }
            AgentOperation::Recover { id, digest } | AgentOperation::Result { id, digest, .. } => {
                super::super::key(id)?;
                if digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(invalid());
                }
            }
        }
        Ok(Self(op))
    }
    pub(in crate::console::native) fn request(
        self,
    ) -> Result<(reqwest::Method, String, Option<Value>), OwnerError> {
        let (suffix, body) = match self.0 {
            AgentOperation::Poll { after } => ("poll".into(), json!({"after":after})),
            AgentOperation::Recover { id, digest } => {
                (format!("{id}/recover"), json!({"commandDigest":digest}))
            }
            AgentOperation::Result {
                id,
                digest,
                token,
                receipt,
            } => (
                format!("{id}/result"),
                json!({"commandDigest":digest,"token":token,"receipt":receipt}),
            ),
        };
        if serde_json::to_vec(&body).map_or(true, |v| v.len() > 16 * 1024) {
            return Err(OwnerError {
                status: 400,
                code: "invalid_managed_agent_payload".into(),
            });
        }
        Ok((
            reqwest::Method::POST,
            format!("/hagency/api/v1/management/agent-executor/{suffix}"),
            Some(body),
        ))
    }
}
