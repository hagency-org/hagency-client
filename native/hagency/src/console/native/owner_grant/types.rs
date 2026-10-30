/// Values chosen in trusted native UI. No identity, command ID, grant ID,
/// accounting month or raw receipt may be supplied by a script.
#[derive(Clone)]
pub struct OwnerGrantChoices {
    pub coordinator_mxid: String,
    pub delegation_expires_at_ms: u64,
    pub allow_self_approval: bool,
    pub eligible_managers: Vec<String>,
    pub allocated_tokens: u64,
}

/// Which successor of a published grant: next month's renewal or this
/// month's amendment of the same pool's capacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerGrantSuccessionKind {
    Renewal,
    Amendment,
}
impl OwnerGrantSuccessionKind {
    fn wire(self) -> &'static str {
        match self {
            Self::Renewal => SUCCESSION_RENEWAL,
            Self::Amendment => SUCCESSION_AMENDMENT,
        }
    }
}
/// Native state of a successor record: pending until the server's acceptance
/// is observed, then effective; rejected or (a never-started renewal)
/// cancelled successors never held capacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerGrantSuccessionState {
    Pending,
    Effective,
    Rejected,
    Cancelled,
}
impl OwnerGrantSuccessionState {
    fn from_native(state: &str) -> Result<Self, NativeError> {
        Ok(match state {
            "pending" => Self::Pending,
            "effective" => Self::Effective,
            "rejected" => Self::Rejected,
            "cancelled" => Self::Cancelled,
            _ => return Err(history_error()),
        })
    }
}
/// The predecessor a successor record names, with the hold it carries.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerGrantSuccessionSummary {
    pub kind: OwnerGrantSuccessionKind,
    pub predecessor_grant_id: String,
    pub predecessor_allocated_tokens: u64,
    pub carried_children: u64,
    pub carried_retained_tokens: u64,
}
/// Live Agents a successor carries and the hold its amount must cover.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerGrantFloor {
    pub children: u64,
    pub retained_tokens: u64,
}
/// Read-only facts for the native successor form. Creates nothing.
#[derive(Clone, Debug)]
pub struct OwnerGrantSuccessionForm {
    pub kind: OwnerGrantSuccessionKind,
    pub contribution_id: String,
    pub resource_id: String,
    pub label: String,
    /// `remaining_tokens` is the most the successor may hold (amendment: with
    /// the predecessor's hold replaced; renewal: in the new month).
    pub context: OwnerGrantReviewContext,
    pub predecessor: OwnerGrantSummary,
    pub floor: OwnerGrantFloor,
    pub period_key: String,
    pub period_start_ms: u64,
    pub period_end_ms: u64,
    /// Renewal window by native UTC clock; amendment: [now, period end).
    pub window_opens_at_ms: u64,
    pub window_closes_at_ms: u64,
}

/// Safe native confirmation summary; the proof and private identity stay in
/// the opaque review handle. The period is the registry's frozen UTC month.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerGrantSummary {
    pub intent_id: String,
    /// The original grant id this intent reserves or reserved.
    pub grant_id: String,
    pub owner_mxid: String,
    pub contribution_id: String,
    pub resource_id: String,
    pub coordinator_mxid: String,
    pub allocated_tokens: u64,
    pub eligible_managers: Vec<String>,
    pub allow_self_approval: bool,
    pub delegation_expires_at_ms: u64,
    pub period_key: String,
    pub period_start_ms: u64,
    pub period_end_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub succession: Option<OwnerGrantSuccessionSummary>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerGrantState {
    ReviewRequired,
    OutcomeUnknown,
    Recorded,
    Declined,
    ReviewExpired,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerGrantProgress {
    pub summary: OwnerGrantSummary,
    pub state: OwnerGrantState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closure: Option<OwnerGrantClosure>,
    /// Native state of a successor record; `None` for a root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub succession_state: Option<OwnerGrantSuccessionState>,
}

/// Descriptive canonical settlement; it grants no authority to reserve again.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerGrantClosure {
    pub retained_tokens: u64,
    pub known_consumed_tokens: u64,
    pub unresolved_tokens: u64,
    pub observed_at_ms: u64,
}

/// Native-derived facts captured with a review. They are descriptive, not a
/// credential. Available capacity is rechecked inside the grant transaction.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerGrantReviewContext {
    pub server_origin: String,
    pub profile_id: String,
    pub resource_revision: String,
    pub framework: String,
    pub model: String,
    pub remaining_tokens: Option<u64>,
    pub resource_total_tokens: Option<u64>,
    pub shared_account_total_tokens: Option<u64>,
    pub observed_at_ms: u64,
}

/// Read-only facts for a native publication form. Opening the form creates no
/// reservation, grant intent, provider login or server publication.
#[derive(Clone, Debug)]
pub struct OwnerGrantForm {
    pub contribution_id: String,
    pub resource_id: String,
    pub label: String,
    pub context: OwnerGrantReviewContext,
}

/// Not Clone or Deserialize. Only prepare/review_existing can produce this,
/// and only its exact originating management instance may consume it.
pub struct OwnerGrantReview {
    manager: Arc<NativeManagement>,
    identity: Identity,
    original: Original,
    admission: admission::AdmissionRound,
    consent: OwnerGrantConsent,
    context: OwnerGrantReviewContext,
    /// A new UTC month on an admission whose earlier months ended.
    regrant: bool,
}
impl OwnerGrantReview {
    pub fn summary(&self) -> OwnerGrantSummary {
        self.original.summary()
    }
    pub fn context(&self) -> &OwnerGrantReviewContext {
        &self.context
    }
    pub fn capacity_available(&self) -> bool {
        self.context.resource_total_tokens.is_some()
            && self
                .context
                .remaining_tokens
                .is_some_and(|n| n >= self.original.selection.allocated_tokens)
    }
    pub fn expired(&self) -> bool {
        self.manager.is_retired()
            || self.consent.binding().is_err()
            || now_ms().map_or(true, |now| now >= self.original.period.end_ms)
    }
}

pub enum OwnerGrantReviewResult {
    Review(Box<OwnerGrantReview>),
    Progress(Box<OwnerGrantProgress>),
}

// These are private journal data, not deserializable execution authority.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Period {
    key: String,
    start_ms: u64,
    end_ms: u64,
}
impl Period {
    fn from_native(value: &OwnerGrantPeriod) -> Self {
        Self {
            key: value.period_key().into(),
            start_ms: value.period_start_ms(),
            end_ms: value.period_end_ms(),
        }
    }
    fn validate(&self) -> Result<(), NativeError> {
        let bytes = self.key.as_bytes();
        let month = self.key.get(5..).and_then(|v| v.parse::<u8>().ok());
        if bytes.len() != 7
            || bytes[4] != b'-'
            || !bytes[..4].iter().all(u8::is_ascii_digit)
            || !month.is_some_and(|m| (1..=12).contains(&m))
            || self.start_ms == 0
            || self.end_ms > hagency_core::JSON_SAFE_MAX
            || !(28 * 86_400_000..=31 * 86_400_000)
                .contains(&self.end_ms.saturating_sub(self.start_ms))
        {
            return Err(history_error());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Selection {
    command_id: String,
    grant_id: String,
    coordinator_mxid: String,
    delegation_expires_at_ms: u64,
    allow_self_approval: bool,
    eligible_managers: Vec<String>,
    allocated_tokens: u64,
}
impl Selection {
    fn native(&self) -> OwnerGrantSelection {
        OwnerGrantSelection {
            command_id: self.command_id.clone(),
            grant_id: self.grant_id.clone(),
            coordinator_mxid: self.coordinator_mxid.clone(),
            delegation_expires_at_ms: self.delegation_expires_at_ms,
            allow_self_approval: self.allow_self_approval,
            eligible_managers: self.eligible_managers.clone(),
            allocated_tokens: self.allocated_tokens,
        }
    }
    fn from_native(value: &OwnerGrantSelection) -> Self {
        Self {
            command_id: value.command_id.clone(),
            grant_id: value.grant_id.clone(),
            coordinator_mxid: value.coordinator_mxid.clone(),
            delegation_expires_at_ms: value.delegation_expires_at_ms,
            allow_self_approval: value.allow_self_approval,
            eligible_managers: value.eligible_managers.clone(),
            allocated_tokens: value.allocated_tokens,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Binding {
    registry_id: String,
    profile_digest: String,
    owner_mxid: String,
    enrollment_id: String,
    contribution_id: String,
    resource_id: String,
    resource_revision: String,
    admitted_coordinator: String,
    executor_device_id: String,
    executor_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verified_resource_binding: Option<String>,
}
impl Binding {
    fn from_native(value: &OwnerGrantBinding) -> Self {
        Self {
            registry_id: value.registry_id.clone(),
            profile_digest: value.profile_digest.clone(),
            owner_mxid: value.owner_mxid.clone(),
            enrollment_id: value.enrollment_id.clone(),
            contribution_id: value.contribution_id.clone(),
            resource_id: value.resource_id.clone(),
            resource_revision: value.resource_revision.clone(),
            admitted_coordinator: value.admitted_coordinator.clone(),
            executor_device_id: value.executor_device_id.clone(),
            executor_generation: value.executor_generation,
            verified_resource_binding: value.verified_resource_binding.clone(),
        }
    }
}
/// A successor intent's immutable link to the grant it succeeds, with the
/// facts the owner reviewed. The receipt's own carried values are recorded by
/// the writer; these are what the native form showed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OriginalSuccession {
    kind: OwnerGrantSuccessionKind,
    predecessor_intent_id: String,
    predecessor_grant_id: String,
    predecessor_receipt_digest: String,
    predecessor_grant_revision: u64,
    predecessor_allocated_tokens: u64,
    reviewed_children: u64,
    reviewed_floor_tokens: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Original {
    intent_id: String,
    binding: Binding,
    // Omit for original journals so their immutable digests remain valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    admission: Option<admission::AdmissionRound>,
    #[serde(default = "legacy_receipt_version", skip_serializing_if = "is_legacy_receipt")]
    receipt_version: u16,
    selection: Selection,
    period: Period,
    // Present exactly on v3 successor intents; omitted otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    succession: Option<OriginalSuccession>,
}
fn legacy_receipt_version() -> u16 { 1 }
fn is_legacy_receipt(version: &u16) -> bool { *version == 1 }
impl Original {
    fn summary(&self) -> OwnerGrantSummary {
        OwnerGrantSummary {
            intent_id: self.intent_id.clone(),
            grant_id: self.selection.grant_id.clone(),
            owner_mxid: self.binding.owner_mxid.clone(),
            contribution_id: self.binding.contribution_id.clone(),
            resource_id: self.binding.resource_id.clone(),
            coordinator_mxid: self.selection.coordinator_mxid.clone(),
            allocated_tokens: self.selection.allocated_tokens,
            eligible_managers: self.selection.eligible_managers.clone(),
            allow_self_approval: self.selection.allow_self_approval,
            delegation_expires_at_ms: self.selection.delegation_expires_at_ms,
            period_key: self.period.key.clone(),
            period_start_ms: self.period.start_ms,
            period_end_ms: self.period.end_ms,
            succession: self.succession.as_ref().map(|s| OwnerGrantSuccessionSummary {
                kind: s.kind,
                predecessor_grant_id: s.predecessor_grant_id.clone(),
                predecessor_allocated_tokens: s.predecessor_allocated_tokens,
                carried_children: s.reviewed_children,
                carried_retained_tokens: s.reviewed_floor_tokens,
            }),
        }
    }
    fn validate(&self, identity: &Identity) -> Result<(), NativeError> {
        if !matches!(self.receipt_version, 1 | 2 | 3)
            || (self.receipt_version >= 2 && self.admission.is_none())
            || (self.receipt_version == 3) != self.succession.is_some()
        {
            return Err(history_error());
        }
        if let Some(s) = &self.succession {
            super::key(&s.predecessor_intent_id)?;
            super::key(&s.predecessor_grant_id)?;
            hash(&s.predecessor_receipt_digest)?;
            positive(s.predecessor_grant_revision)?;
            positive(s.predecessor_allocated_tokens)?;
            if s.predecessor_grant_id == self.selection.grant_id
                || s.predecessor_intent_id == self.intent_id
                || s.reviewed_children > hagency_core::JSON_SAFE_MAX
                || s.reviewed_floor_tokens > hagency_core::JSON_SAFE_MAX
            {
                return Err(history_error());
            }
        }
        if let Some(admission) = &self.admission {
            admission.validate()?;
        }
        let b = &self.binding;
        let s = &self.selection;
        for value in [
            &self.intent_id,
            &s.command_id,
            &s.grant_id,
            &b.enrollment_id,
            &b.contribution_id,
            &b.resource_id,
            &b.executor_device_id,
        ] {
            super::key(value)?;
        }
        for value in [&b.registry_id, &b.profile_digest, &b.resource_revision] {
            hash(value)?;
        }
        if let Some(value) = &b.verified_resource_binding {
            hash(value)?;
        }
        self.period.validate()?;
        positive(b.executor_generation)?;
        positive(s.allocated_tokens)?;
        positive(s.delegation_expires_at_ms)?;
        if b.owner_mxid != identity.owner
            || b.executor_device_id != identity.device
            || b.profile_digest != profile_digest(identity)?
            || b.admitted_coordinator != s.coordinator_mxid
            || s.eligible_managers.is_empty()
            || s.eligible_managers.len() > 64
            || s.eligible_managers.windows(2).any(|p| p[0] >= p[1])
        {
            return Err(history_error());
        }
        for mxid in std::iter::once(&s.coordinator_mxid).chain(&s.eligible_managers) {
            same_server_user(mxid, &identity.owner)?;
        }
        Ok(())
    }
    fn matches_receipt(&self, receipt: &NativeGrantReceipt) -> bool {
        let b = &self.binding;
        let s = &self.selection;
        let succession = match (&self.succession, &receipt.succession) {
            (None, None) => receipt.grant_revision == 1,
            (Some(o), Some(r)) => {
                receipt.grant_revision == o.predecessor_grant_revision.saturating_add(1)
                    && r.kind == o.kind.wire()
                    && r.predecessor_grant_id == o.predecessor_grant_id
                    && r.predecessor_receipt_digest == o.predecessor_receipt_digest
                    && receipt.supported_shape()
            }
            _ => false,
        };
        succession
            && receipt.version == self.receipt_version
            && receipt.admission_request_id.as_deref() == if self.receipt_version >= 2 {
                self.admission.as_ref().map(|a| a.request_id.as_str())
            } else { None }
            && receipt.capability == "hagency.project_reservations.v1"
            && receipt.enrollment_id == b.enrollment_id
            && receipt.grant_id == s.grant_id
            && receipt.contribution_id == b.contribution_id
            && receipt.resource_id == b.resource_id
            && receipt.resource_revision == b.resource_revision
            && receipt.executor_device_id == b.executor_device_id
            && receipt.executor_generation == b.executor_generation
            && receipt.owner_mxid == b.owner_mxid
            && receipt.coordinator_mxid == s.coordinator_mxid
            && receipt.delegation_revision == 1
            && receipt.delegation_expires_at_ms == s.delegation_expires_at_ms
            && receipt.allow_self_approval == s.allow_self_approval
            && receipt.eligible_managers == s.eligible_managers
            && receipt.allocated_tokens == s.allocated_tokens
            && receipt.period_key == self.period.key
            && receipt.period_start_ms == self.period.start_ms
            && receipt.period_end_ms == self.period.end_ms
            && receipt.ledger_revision > 0
            && receipt.ledger_revision <= hagency_core::JSON_SAFE_MAX
    }
}
