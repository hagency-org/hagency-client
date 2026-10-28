#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetupBinding {
    pub revision: u64,
    pub allocation_id: String,
    pub allocation_receipt_digest: String,
    pub project_revision: u64,
    pub authority_revision: u64,
    pub administrator_mxid: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectAdministrator {
    pub mxid: String,
    pub revision: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectSetup {
    pub id: String,
    pub project_id: String,
    pub allocation_id: String,
    pub allocation_receipt_digest: String,
    pub project_revision: u64,
    pub authority_revision: u64,
    pub definition_digest: String,
    pub mode: String,
    pub state: String,
    pub revision: u64,
    pub name: String,
    pub manager_mxid: String,
    pub administrator_mxid: String,
    pub space_creation_command_id: String,
    pub room_creation_command_id: String,
    pub space_id: Option<String>,
    pub room_id: Option<String>,
    pub domain_project_id: Option<String>,
    pub verified_at_ms: Option<u64>,
    #[serde(default)]
    pub current_binding: Option<SetupBinding>,
}
/// Exact live reservation selected by the server, independently of an existing
/// setup's immutable original allocation and current binding.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EligibleSetupAllocation {
    pub id: String,
    pub revision: u64,
    pub project_revision: u64,
    pub receipt_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectReadiness {
    pub project_id: String,
    pub administrator: Option<ProjectAdministrator>,
    pub setup: Option<ProjectSetup>,
    #[serde(default)]
    pub eligible_allocation: Option<EligibleSetupAllocation>,
    pub capacity_ready: bool,
    pub matrix_ready: bool,
    pub can_assign_administrator: bool,
    pub can_begin: bool,
    pub can_confirm: bool,
    #[serde(default)]
    pub can_cancel: bool,
    #[serde(default)]
    pub can_rebase: bool,
    pub blocking_reason: Option<String>,
    pub observed_at_ms: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BeginSetup {
    pub project_id: String,
    pub allocation_id: String,
    pub expected_project_revision: u64,
    pub expected_allocation_revision: u64,
    pub expected_authority_revision: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignAdministrator {
    pub project_id: String,
    pub expected_authority_revision: u64,
    pub administrator_mxid: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RebaseSetup {
    pub project_id: String,
    pub setup_id: String,
    pub expected_revision: u64,
    pub allocation_id: String,
    pub expected_project_revision: u64,
    pub expected_allocation_revision: u64,
    pub expected_authority_revision: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetupCandidates {
    pub space_id: Option<String>,
    pub room_id: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupProgress {
    pub readiness: ProjectReadiness,
    pub local_history: bool,
    pub space: Option<StageProgress>,
    pub room: Option<StageProgress>,
    pub pending_commands: Vec<String>,
    pub failed_commands: Vec<FailedCommand>,
    pub blocker: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StageProgress {
    pub state: String,
    pub room_id: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedCommand {
    pub command_id: String,
    pub code: String,
}

impl ProjectSetup {
    fn definition(&self) -> Value {
        json!({"version":1,"setupId":self.id,"projectId":self.project_id,
        "projectRevision":self.project_revision,"allocationId":self.allocation_id,
        "allocationReceiptDigest":self.allocation_receipt_digest,"managerMxid":self.manager_mxid,
        "administratorMxid":self.administrator_mxid,"authorityRevision":self.authority_revision,
        "name":self.name,"mode":self.mode})
    }
    fn same_original(&self, other: &Self) -> bool {
        self.definition() == other.definition()
            && self.definition_digest == other.definition_digest
            && self.space_creation_command_id == other.space_creation_command_id
            && self.room_creation_command_id == other.room_creation_command_id
    }
    fn validate(&self) -> Result<(), NativeError> {
        for s in [
            &self.id,
            &self.project_id,
            &self.allocation_id,
            &self.space_creation_command_id,
            &self.room_creation_command_id,
        ] {
            key(s)?;
        }
        for n in [
            self.project_revision,
            self.authority_revision,
            self.revision,
        ] {
            positive(n)?;
        }
        for s in [&self.allocation_receipt_digest, &self.definition_digest] {
            hash(s)?;
        }
        for s in [&self.manager_mxid, &self.administrator_mxid] {
            mxid(s)?;
        }
        if self.name.trim().is_empty()
            || self.name.chars().count() > 128
            || self.name.chars().any(char::is_control)
            || !matches!(self.mode.as_str(), "create" | "adopt")
            || !matches!(
                self.state.as_str(),
                "awaiting_matrix_setup" | "ready" | "cancelled"
            )
            || self.space_creation_command_id == self.room_creation_command_id
            || digest(&self.definition())? != self.definition_digest
        {
            return Err(invalid());
        }
        for s in [&self.space_id, &self.room_id].into_iter().flatten() {
            room_id(s)?;
        }
        if self.space_id.is_some() && self.space_id == self.room_id {
            return Err(invalid());
        }
        if let Some(id) = &self.domain_project_id {
            key(id)?;
        }
        if let Some(n) = self.verified_at_ms {
            positive(n)?;
        }
        if let Some(b) = &self.current_binding {
            for n in [b.revision, b.project_revision, b.authority_revision] {
                positive(n)?;
            }
            key(&b.allocation_id)?;
            hash(&b.allocation_receipt_digest)?;
            mxid(&b.administrator_mxid)?;
        }
        Ok(())
    }
}
impl ProjectReadiness {
    fn validate(&self, project: &str) -> Result<(), NativeError> {
        key(project)?;
        positive(self.observed_at_ms)?;
        if self.project_id != project
            || self.blocking_reason.as_ref().is_some_and(|s| s.len() > 128)
        {
            return Err(invalid());
        }
        if let Some(a) = &self.administrator {
            mxid(&a.mxid)?;
            positive(a.revision)?;
        }
        if let Some(a) = &self.eligible_allocation {
            key(&a.id)?;
            positive(a.revision)?;
            positive(a.project_revision)?;
            hash(&a.receipt_digest)?;
        }
        if let Some(s) = &self.setup {
            s.validate()?;
            if s.project_id != project {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
