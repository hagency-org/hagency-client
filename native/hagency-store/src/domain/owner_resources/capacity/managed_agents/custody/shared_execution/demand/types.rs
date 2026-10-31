use super::*;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedDemandOffer {
    pub version: u16,
    pub demand_id: String,
    pub assignment_id: String,
    pub event_id: String,
    pub event_digest: String,
    pub control_revision: u64,
    pub received_at_ms: u64,
    pub predecessor_command_id: String,
    pub predecessor_command_digest: String,
}
impl SharedDemandOffer {
    pub fn validate(&self) -> Result<(), Error> {
        for id in [
            &self.demand_id,
            &self.assignment_id,
            &self.predecessor_command_id,
        ] {
            identifier(id, 128)?;
        }
        hash(&self.event_digest)?;
        hash(&self.predecessor_command_digest)?;
        exact(self.control_revision)?;
        exact(self.received_at_ms)?;
        if self.version != 1
            || !self.event_id.starts_with('$')
            || self.event_id.len() < 2
            || self.event_id.len() > 255
            || self.event_id.chars().any(char::is_control)
            || serde_json::to_vec(self)?.len() > 8192
        {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
    pub fn digest(&self) -> Result<String, Error> {
        self.validate()?;
        Ok(canonical::digest(&serde_json::to_value(self)?)?)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedDemandStartInput {
    pub demand_digest: String,
    pub expected_control_revision: u64,
    pub start: SharedAttemptStartInput,
}
impl SharedDemandStartInput {
    pub fn validate_for(&self, offer: &SharedDemandOffer) -> Result<(), Error> {
        offer.validate()?;
        identifier(&self.start.start_intent_id, 128)?;
        exact(self.start.predecessor.process_attempt)?;
        hash(&self.start.predecessor.stopped_receipt_digest)?;
        hash(&self.start.predecessor.process_digest)?;
        let b = &self.start.budget_observation;
        validate_budget(b)?;
        if b.remaining_tokens == 0 {
            return Err(Error::Capacity);
        }
        if self.demand_digest != offer.digest()?
            || self.expected_control_revision != offer.control_revision
            || self.start.assignment_id != offer.assignment_id
            || self.start.predecessor.command_id != offer.predecessor_command_id
            || self.start.predecessor.command_digest != offer.predecessor_command_digest
            || serde_json::to_vec(self)?.len() > 16384
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedDemandBinding {
    pub version: u16,
    pub demand_id: String,
    pub demand_digest: String,
    pub control_revision: u64,
    pub assignment_id: String,
    pub event_id: String,
    pub received_at_ms: u64,
    pub runtime_command_id: String,
    pub runtime_command_digest: String,
    pub work_id: String,
    pub work_digest: String,
}
impl SharedDemandBinding {
    pub fn validate_offer(&self, offer: &SharedDemandOffer) -> Result<(), Error> {
        offer.validate()?;
        identifier(&self.runtime_command_id, 128)?;
        identifier(&self.work_id, 128)?;
        hash(&self.runtime_command_digest)?;
        hash(&self.work_digest)?;
        if self.version != 1
            || self.demand_id != offer.demand_id
            || self.demand_digest != offer.digest()?
            || self.control_revision != offer.control_revision
            || self.assignment_id != offer.assignment_id
            || self.event_id != offer.event_id
            || self.received_at_ms != offer.received_at_ms
            || serde_json::to_vec(self)?.len() + serde_json::to_vec(offer)?.len() > 16384
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    pub fn validate_for(
        &self,
        offer: &SharedDemandOffer,
        work: &SharedWorkItem,
    ) -> Result<(), Error> {
        self.validate_offer(offer)?;
        work.validate()?;
        identifier(&self.runtime_command_id, 128)?;
        hash(&self.runtime_command_digest)?;
        if self.version != 1
            || self.demand_id != offer.demand_id
            || self.demand_digest != offer.digest()?
            || self.control_revision != offer.control_revision
            || self.assignment_id != offer.assignment_id
            || work.assignment_id != offer.assignment_id
            || self.event_id != offer.event_id
            || work.event_id != offer.event_id
            || self.received_at_ms != offer.received_at_ms
            || work.created_at_ms != offer.received_at_ms
            || self.runtime_command_digest != work.runtime_command_digest
            || self.work_id != work.id
            || self.work_digest != canonical::digest(&serde_json::to_value(work)?)?
            || serde_json::to_vec(self)?.len() + serde_json::to_vec(offer)?.len() > 16384
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}
/// Current authenticated descriptive offer. This handle confers no process or
/// model permission, and cannot be reconstructed from a saved JSON request.
pub struct VerifiedSharedDemand {
    pub(in super::super) enrollment: VerifiedManagementEnrollment,
    pub(in super::super) offer: SharedDemandOffer,
    pub(in super::super) expires: Instant,
}
impl VerifiedSharedDemand {
    pub fn new(
        enrollment: VerifiedManagementEnrollment,
        offer: SharedDemandOffer,
        digest: String,
        observed_at_ms: u64,
        expires: Instant,
    ) -> Result<Self, Error> {
        enrollment.check()?;
        exact(observed_at_ms)?;
        let now = Instant::now();
        if offer.digest()? != digest
            || observed_at_ms < offer.received_at_ms
            || observed_at_ms - offer.received_at_ms > 24 * 60 * 60 * 1000
            || expires <= now
            || expires > now + Duration::from_secs(30)
        {
            return Err(Error::LocalAuthority);
        }
        let expires = expires.min(enrollment.expires);
        let value = Self {
            enrollment,
            offer,
            expires,
        };
        value.check()?;
        Ok(value)
    }
    pub(in super::super) fn check(&self) -> Result<(), Error> {
        self.enrollment.check()?;
        if Instant::now() >= self.expires {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SharedDemandDisposition {
    Pending,
    StartPrepared,
    BudgetBlocked,
    HeadChanged,
}
/// Immutable original request/data only. Recovery does not return live proof.
#[derive(Clone)]
pub struct SharedDemandIntent {
    pub(super) offer: SharedDemandOffer,
    pub(super) disposition: SharedDemandDisposition,
    pub(super) input: Option<SharedDemandStartInput>,
    pub(super) budget: Option<SharedDemandBudget>,
}
impl SharedDemandIntent {
    pub fn offer(&self) -> &SharedDemandOffer {
        &self.offer
    }
    pub fn disposition(&self) -> &SharedDemandDisposition {
        &self.disposition
    }
    pub fn input(&self) -> Option<&SharedDemandStartInput> {
        self.input.as_ref()
    }
    pub fn budget(&self) -> Option<&SharedDemandBudget> {
        self.budget.as_ref()
    }
}

/// Saved budget description is data only; it cannot construct a live budget or
/// a call/start authority. Fresh writers always recompute canonical liability.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedDemandBudget {
    pub state: String,
    pub observation: Option<SharedBudgetObservation>,
}
impl From<&SharedSuccessorBudget> for SharedDemandBudget {
    fn from(b: &SharedSuccessorBudget) -> Self {
        Self {
            state: match b.state {
                SharedSuccessorBudgetState::Available => "available",
                SharedSuccessorBudgetState::Exhausted => "exhausted",
                SharedSuccessorBudgetState::Unrepresentable => "unrepresentable",
            }
            .into(),
            observation: Some(SharedBudgetObservation {
                observed_ledger_revision: b.observed_ledger_revision,
                allocated_tokens: b.allocated_tokens,
                known_consumed_tokens: b.known_consumed_tokens,
                retained_tokens: b.retained_tokens,
                remaining_tokens: b.remaining_tokens,
                usage_complete: b.usage_complete,
                liability_digest: b.liability_digest.clone(),
            }),
        }
    }
}

impl SharedDemandBudget {
    pub(super) fn unavailable() -> Self {
        Self {
            state: "unrepresentable".into(),
            observation: None,
        }
    }
    pub(super) fn validate(&self) -> Result<(), Error> {
        match (&*self.state, &self.observation) {
            ("exhausted", Some(b)) => {
                validate_budget(b)?;
                if b.remaining_tokens != 0 {
                    return Err(Error::Conflict);
                }
            }
            ("unrepresentable", Some(b)) => {
                exact(b.observed_ledger_revision)?;
                exact(b.allocated_tokens)?;
                hash(&b.liability_digest)?;
                if b.remaining_tokens != 0
                    || b.usage_complete
                    || b.known_consumed_tokens > JSON_SAFE_MAX
                    || b.retained_tokens > JSON_SAFE_MAX
                    || b.retained_tokens < b.known_consumed_tokens
                {
                    return Err(Error::Conflict);
                }
            }
            ("unrepresentable", None) => {}
            _ => return Err(Error::Conflict),
        }
        Ok(())
    }
}
fn validate_budget(b: &SharedBudgetObservation) -> Result<(), Error> {
    exact(b.observed_ledger_revision)?;
    exact(b.allocated_tokens)?;
    hash(&b.liability_digest)?;
    if b.known_consumed_tokens > JSON_SAFE_MAX
        || b.retained_tokens > JSON_SAFE_MAX
        || b.retained_tokens < b.known_consumed_tokens
        || b.remaining_tokens != b.allocated_tokens.saturating_sub(b.retained_tokens)
        || (b.usage_complete && b.retained_tokens != b.known_consumed_tokens)
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
