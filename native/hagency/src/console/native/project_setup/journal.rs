use std::path::{Path, PathBuf};
const MAX_BYTES: usize = 2 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::console) struct Identity {
    pub origin: String,
    pub owner: String,
    pub issuer: String,
    pub subject: String,
    pub client: String,
    pub installation: String,
    pub device: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StageState {
    NotIssued,
    Submitted,
    Known,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stage {
    state: StageState,
    payload: Option<matrix::Create>,
    room_id: Option<String>,
}
impl Stage {
    fn new() -> Self {
        Self {
            state: StageState::NotIssued,
            payload: None,
            room_id: None,
        }
    }
    fn progress(&self) -> StageProgress {
        StageProgress {
            state: match self.state {
                StageState::NotIssued => "not_issued",
                StageState::Submitted => "outcome_unknown",
                StageState::Known => "room_returned",
            }
            .into(),
            room_id: self.room_id.clone(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Invite {
    binding_revision: u64,
    admin: String,
    room: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    project: String,
    commands: Vec<Command>,
    original: Option<ProjectSetup>,
    space: Option<Stage>,
    room: Option<Stage>,
    invites: Vec<Invite>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    version: u32,
    identity: Identity,
    records: Vec<Record>,
}
struct History {
    path: PathBuf,
    data: Journal,
}
impl History {
    fn load(path: &Path, identity: Identity) -> Result<Self, NativeError> {
        let data = if !path.exists() {
            Journal {
                version: 1,
                identity: identity.clone(),
                records: vec![],
            }
        } else {
            let bytes = crate::console::server_login::read_private_json(path, MAX_BYTES as u64)
                .map_err(|_| history_error())?;
            if bytes.len() > MAX_BYTES {
                return Err(history_error());
            }
            serde_json::from_slice::<Journal>(&bytes).map_err(|_| history_error())?
        };
        if data.version != 1 || data.identity != identity || data.records.len() > 64 {
            return Err(history_error());
        }
        let mut projects = std::collections::BTreeSet::new();
        let mut commands = std::collections::BTreeSet::new();
        for r in &data.records {
            key(&r.project)?;
            if !projects.insert(&r.project) || r.commands.len() > 32 || r.invites.len() > 32 {
                return Err(history_error());
            }
            for c in &r.commands {
                c.validate()?;
                if c.project != r.project || !commands.insert(&c.id) {
                    return Err(history_error());
                }
            }
            if let Some(s) = &r.original {
                s.validate()?;
                if s.project_id != r.project
                    || s.manager_mxid != identity.owner
                    || s.mode != "create"
                    || r.space.is_none()
                    || r.room.is_none()
                    || !r.commands.iter().any(|c| {
                        matches!(c.mutation, Mutation::Begin { .. })
                            && c.outcome == Outcome::Succeeded
                    })
                {
                    return Err(history_error());
                }
            } else if r.space.is_some()
                || r.room.is_some()
                || r.commands.iter().any(|c| {
                    matches!(c.mutation, Mutation::Begin { .. }) && c.outcome == Outcome::Succeeded
                })
            {
                return Err(history_error());
            }
            for stage in [&r.space, &r.room].into_iter().flatten() {
                if !matches!(stage.state, StageState::NotIssued) && stage.payload.is_none() {
                    return Err(history_error());
                }
                if matches!(stage.state, StageState::Known) != stage.room_id.is_some() {
                    return Err(history_error());
                }
                if let Some(id) = &stage.room_id {
                    room_id(id)?;
                }
                if let Some(p) = &stage.payload {
                    p.validate()?;
                    if r.original
                        .as_ref()
                        .is_none_or(|s| !s.same_original(&p.setup))
                    {
                        return Err(history_error());
                    }
                }
            }
            for i in &r.invites {
                positive(i.binding_revision)?;
                mxid(&i.admin)?;
                room_id(&i.room)?;
            }
        }
        Ok(Self {
            path: path.into(),
            data,
        })
    }
    fn save(&self) -> Result<(), NativeError> {
        let bytes = serde_json::to_vec(&self.data).map_err(|_| history_error())?;
        if bytes.len() > MAX_BYTES {
            return Err(failure(409, "local_setup_history_full"));
        }
        hagency_store::private::replace(&self.path, &bytes).map_err(|_| history_error())
    }
    fn index(&self, project: &str) -> Option<usize> {
        self.data.records.iter().position(|r| r.project == project)
    }
    fn add(&mut self, command: Command) -> Result<usize, NativeError> {
        let n = match self.index(&command.project) {
            Some(n) => n,
            None => {
                if self.data.records.len() >= 64 {
                    return Err(failure(409, "local_setup_history_full"));
                }
                self.data.records.push(Record {
                    project: command.project.clone(),
                    commands: vec![],
                    original: None,
                    space: None,
                    room: None,
                    invites: vec![],
                });
                self.data.records.len() - 1
            }
        };
        if self.data.records[n].commands.len() >= 32 {
            return Err(failure(409, "local_setup_history_full"));
        }
        if self.data.records[n]
            .commands
            .iter()
            .any(|c| c.outcome == Outcome::Pending)
        {
            return Err(failure(409, "local_setup_command_pending"));
        }
        self.data.records[n].commands.push(command);
        self.save()?;
        Ok(n)
    }
    fn progress(&self, readiness: ProjectReadiness, blocker: Option<String>) -> SetupProgress {
        let r = self
            .index(&readiness.project_id)
            .map(|n| &self.data.records[n]);
        SetupProgress {
            local_history: r.is_some_and(|r| r.original.is_some()),
            space: r.and_then(|r| r.space.as_ref()).map(Stage::progress),
            room: r.and_then(|r| r.room.as_ref()).map(Stage::progress),
            pending_commands: r
                .map(|r| {
                    r.commands
                        .iter()
                        .filter(|c| c.outcome == Outcome::Pending)
                        .map(|c| c.id.clone())
                        .collect()
                })
                .unwrap_or_default(),
            failed_commands: r
                .map(|r| {
                    r.commands
                        .iter()
                        .filter_map(|c| {
                            if let Outcome::Failed { code, .. } = &c.outcome {
                                Some(FailedCommand {
                                    command_id: c.id.clone(),
                                    code: code.clone(),
                                })
                            } else {
                                None
                            }
                        })
                        .collect()
                })
                .unwrap_or_default(),
            readiness,
            blocker,
        }
    }
}
fn history_error() -> NativeError {
    failure(409, "local_setup_history_required")
}
