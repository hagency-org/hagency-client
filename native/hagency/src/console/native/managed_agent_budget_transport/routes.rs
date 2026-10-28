use super::*;
use crate::console::server_login::OwnerError;
use serde_json::{Value, json};
pub(in crate::console::native) struct ServerOperation(Action);
pub(super) enum Action {
    Discovery,
    Poll {
        after: Option<String>,
    },
    Recover {
        id: String,
        digest: String,
    },
    Result {
        id: String,
        input: Box<AgentBudgetResult>,
    },
}
impl ServerOperation {
    pub(super) fn new(action: Action) -> Result<Self, NativeError> {
        match &action {
            Action::Discovery => {}
            Action::Poll { after } => {
                if let Some(id) = after {
                    super::super::key(id)?
                }
            }
            Action::Recover { id, digest } => {
                super::super::key(id)?;
                hash(digest)?
            }
            Action::Result { id, input } => {
                super::super::key(id)?;
                hash(&input.command_digest)?;
                hash(&input.token)?
            }
        }
        Ok(Self(action))
    }
    pub(in crate::console::native) fn request(
        self,
    ) -> Result<(reqwest::Method, String, Option<Value>), OwnerError> {
        let (suffix, body) = match self.0 {
            Action::Discovery => {
                return Ok((
                    reqwest::Method::GET,
                    "/hagency/api/v1/discovery".into(),
                    None,
                ));
            }
            Action::Poll { after } => ("poll".into(), json!({"after":after})),
            Action::Recover { id, digest } => {
                (format!("{id}/recover"), json!({"commandDigest":digest}))
            }
            Action::Result { id, input } => (format!("{id}/result"), json!(input)),
        };
        if serde_json::to_vec(&body).map_or(true, |v| v.len() > 16 * 1024) {
            return Err(OwnerError {
                status: 400,
                code: "invalid_agent_budget_payload".into(),
            });
        }
        Ok((
            reqwest::Method::POST,
            format!("/hagency/api/v1/management/agent-budget-executor/{suffix}"),
            Some(body),
        ))
    }
}
