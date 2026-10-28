//! Closed v2 route and wire adapter. No generic URL or script-provided authority.
use super::*;
use serde_json::{Value, json};
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Delivery {
    pub command: SharedAttemptCommand,
    pub command_digest: String,
    pub token: String,
    pub lease_until_ms: u64,
    pub execution_allowed: bool,
    pub cleanup_allowed: bool,
    pub recorded: Option<SharedAttemptProcessReceipt>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Reply {
    pub delivery: Option<Delivery>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ResultInput {
    pub command_digest: String,
    pub token: String,
    pub receipt: SharedAttemptProcessReceipt,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Acknowledgement {
    pub observation_id: String,
    pub receipt_digest: String,
    pub activity: Option<SharedActivityLease>,
    pub state: String,
}
pub(super) enum AttemptAction {
    Start(SharedAttemptStartInput),
    DemandStart {
        id: String,
        input: SharedDemandStartInput,
    },
    Poll {
        after: Option<String>,
    },
    Recover {
        id: String,
        digest: String,
        observation: Option<String>,
    },
    Result {
        id: String,
        input: ResultInput,
    },
}
impl AttemptAction {
    pub(super) fn validate(&self) -> Result<(), NativeError> {
        match self {
            Self::DemandStart { id, input } => {
                if !cfg!(feature = "shared-intake") {
                    return Err(failure(409, "shared_intake_disabled"));
                }
                super::super::key(id)?;
                hash(&input.demand_digest)?;
                if input.expected_control_revision == 0
                    || input.expected_control_revision > 9_007_199_254_740_991
                {
                    return Err(invalid());
                }
                Self::Start(input.start.clone()).validate()?;
            }
            Self::Start(v) => {
                super::super::key(&v.start_intent_id)?;
                super::super::key(&v.assignment_id)?;
                super::super::key(&v.predecessor.command_id)?;
                hash(&v.predecessor.command_digest)?;
                hash(&v.predecessor.stopped_receipt_digest)?;
                hash(&v.predecessor.process_digest)?;
                hash(&v.budget_observation.liability_digest)?;
            }
            Self::Poll { after } => {
                if let Some(id) = after {
                    super::super::key(id)?;
                }
            }
            Self::Recover {
                id,
                digest,
                observation,
            } => {
                super::super::key(id)?;
                hash(digest)?;
                if let Some(id) = observation {
                    super::super::key(id)?;
                }
            }
            Self::Result { id, input } => {
                super::super::key(id)?;
                hash(&input.command_digest)?;
                hash(&input.token)?;
            }
        }
        Ok(())
    }
    pub(super) fn body(self) -> (String, Value) {
        match self {
            Self::Start(input) => ("attempts/start".into(), json!(input)),
            Self::DemandStart { id, input } => (format!("demands/{id}/start"), json!(input)),
            Self::Poll { after } => ("attempts/poll".into(), json!({"after":after})),
            Self::Recover {
                id,
                digest,
                observation,
            } => (
                format!("attempts/{id}/recover"),
                json!({"commandDigest":digest,"observationId":observation}),
            ),
            Self::Result { id, input } => (format!("attempts/{id}/result"), json!(input)),
        }
    }
}
impl From<AttemptAction> for Action {
    fn from(v: AttemptAction) -> Self {
        Action::Attempt(Box::new(v))
    }
}
