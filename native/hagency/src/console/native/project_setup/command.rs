#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Mutation {
    Assign {
        expected_authority_revision: u64,
        administrator_mxid: String,
    },
    Begin {
        // Historical journals omitted the selection. Preserve their exact
        // serialization for recorded-result recovery, never a new begin POST.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allocation_id: Option<String>,
        expected_project_revision: u64,
        expected_allocation_revision: u64,
        expected_authority_revision: u64,
    },
    Cancel {
        setup_id: String,
        expected_revision: u64,
    },
    Rebase {
        setup_id: String,
        expected_revision: u64,
        allocation_id: String,
        expected_project_revision: u64,
        expected_allocation_revision: u64,
        expected_authority_revision: u64,
    },
    Confirm {
        setup_id: String,
        expected_revision: u64,
        space_id: String,
        room_id: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Outcome {
    Pending,
    Succeeded,
    Failed { status: u16, code: String },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Command {
    pub id: String,
    pub project: String,
    pub mutation: Mutation,
    pub attempted: bool,
    pub outcome: Outcome,
}
impl Command {
    fn new(project: String, mutation: Mutation) -> Result<Self, NativeError> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| failure(503, "native_random_unavailable"))?;
        let id = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let c = Self {
            id,
            project,
            mutation,
            attempted: false,
            outcome: Outcome::Pending,
        };
        c.validate()?;
        Ok(c)
    }
    fn operation(&self) -> &'static str {
        match self.mutation {
            Mutation::Assign { .. } => "project.admin.assign",
            Mutation::Begin { .. } => "project.setup.begin",
            Mutation::Cancel { .. } => "project.setup.cancel",
            Mutation::Rebase { .. } => "project.setup.rebase",
            Mutation::Confirm { .. } => "project.setup.confirm",
        }
    }
    fn permission(&self) -> Permission {
        if matches!(self.mutation, Mutation::Assign { .. }) {
            Permission::AssignProjectAdministrators
        } else {
            Permission::ManageProjectSetup
        }
    }
    fn validate(&self) -> Result<(), NativeError> {
        key(&self.id)?;
        key(&self.project)?;
        match &self.mutation {
            Mutation::Assign {
                expected_authority_revision,
                administrator_mxid,
            } => {
                safe(*expected_authority_revision)?;
                mxid(administrator_mxid)?;
            }
            Mutation::Begin {
                allocation_id,
                expected_project_revision,
                expected_allocation_revision,
                expected_authority_revision,
            } => {
                if let Some(id) = allocation_id {
                    key(id)?;
                }
                positive(*expected_project_revision)?;
                positive(*expected_allocation_revision)?;
                positive(*expected_authority_revision)?;
            }
            Mutation::Cancel {
                setup_id,
                expected_revision,
            } => {
                key(setup_id)?;
                positive(*expected_revision)?;
            }
            Mutation::Rebase {
                setup_id,
                expected_revision,
                allocation_id,
                expected_project_revision,
                expected_allocation_revision,
                expected_authority_revision,
            } => {
                key(setup_id)?;
                key(allocation_id)?;
                for n in [
                    expected_revision,
                    expected_project_revision,
                    expected_allocation_revision,
                    expected_authority_revision,
                ] {
                    positive(*n)?;
                }
            }
            Mutation::Confirm {
                setup_id,
                expected_revision,
                space_id,
                room_id: room,
            } => {
                key(setup_id)?;
                positive(*expected_revision)?;
                room_id(space_id)?;
                room_id(room)?;
                if space_id == room {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
    fn route(&self) -> (&'static str, String, Value) {
        let id = &self.id;
        match &self.mutation {
            Mutation::Assign {
                expected_authority_revision,
                administrator_mxid,
            } => (
                "PUT",
                format!("projects/{}/administrator", self.project),
                json!({"commandId":id,"expectedAuthorityRevision":expected_authority_revision,"administratorMxid":administrator_mxid}),
            ),
            Mutation::Begin {
                allocation_id,
                expected_project_revision,
                expected_allocation_revision,
                expected_authority_revision,
            } => {
                let mut body = json!({"commandId":id,"expectedProjectRevision":expected_project_revision,"expectedAllocationRevision":expected_allocation_revision,"expectedAuthorityRevision":expected_authority_revision,"mode":"create"});
                if let Some(allocation) = allocation_id {
                    body["allocationId"] = json!(allocation);
                }
                ("POST", format!("projects/{}/setup", self.project), body)
            }
            Mutation::Cancel {
                setup_id,
                expected_revision,
            } => (
                "POST",
                format!("project-setups/{setup_id}/cancel"),
                json!({"commandId":id,"expectedRevision":expected_revision}),
            ),
            Mutation::Rebase {
                setup_id,
                expected_revision,
                allocation_id,
                expected_project_revision,
                expected_allocation_revision,
                expected_authority_revision,
            } => (
                "POST",
                format!("project-setups/{setup_id}/rebase"),
                json!({"commandId":id,"expectedRevision":expected_revision,"allocationId":allocation_id,"expectedProjectRevision":expected_project_revision,"expectedAllocationRevision":expected_allocation_revision,"expectedAuthorityRevision":expected_authority_revision}),
            ),
            Mutation::Confirm {
                setup_id,
                expected_revision,
                space_id,
                room_id,
            } => (
                "POST",
                format!("project-setups/{setup_id}/confirm"),
                json!({"commandId":id,"expectedRevision":expected_revision,"spaceId":space_id,"roomId":room_id}),
            ),
        }
    }
    fn no_effect(&self, code: &str) -> bool {
        // Server-audited transaction errors, only before the first POST returns.
        // Authentication, command conflict, transport and post-write failures excluded.
        match self.mutation {
            Mutation::Begin { .. } => matches!(
                code,
                "project_administrator_required"
                    | "project_setup_allocation_required"
                    | "live_reservation_required"
                    | "management_revision_changed"
                    | "project_setup_already_exists"
            ),
            Mutation::Assign { .. } => {
                matches!(code, "project_unavailable" | "management_revision_changed" | "project_manager_is_administrator")
            }
            Mutation::Cancel { .. } => matches!(
                code,
                "management_revision_changed" | "project_setup_cancelled"
            ),
            Mutation::Rebase { .. } => matches!(
                code,
                "project_unavailable"
                    | "management_revision_changed"
                    | "project_administrator_required"
                    | "live_reservation_required"
            ),
            Mutation::Confirm { .. } => matches!(
                code,
                "management_revision_changed"
                    | "project_setup_cancelled"
                    | "project_administrator_required"
                    | "project_administrator_changed"
                    | "project_matrix_proof_missing"
                    | "project_matrix_proof_conflict"
                    | "project_matrix_marker_mismatch"
                    | "project_membership_pending"
                    | "project_private_room_required"
                    | "project_setup_changed"
                    | "project_matrix_link_immutable"
                    | "live_reservation_required"
                    | "project_matrix_not_ready"
                    | "project_disabled"
                    | "managed_project_has_bindings"
                    | "room_disabled"
            ),
        }
    }
}
