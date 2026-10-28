trait Io {
    fn owner(&self) -> &str;
    async fn check(&self) -> Result<(), NativeError>;
    async fn read(&self, project: &str) -> Result<ProjectReadiness, NativeError>;
    async fn command(&self, command: &Command) -> Result<ProjectReadiness, NativeError>;
    async fn recover(&self, command: &Command) -> Result<Option<ProjectReadiness>, NativeError>;
    async fn matrix(&self, operation: matrix::Operation) -> Result<Value, NativeError>;
}
struct Driver<I> {
    io: I,
    history: History,
}
impl<I: Io> Driver<I> {
    async fn progress(&self, project: &str) -> Result<SetupProgress, NativeError> {
        let r = self.io.read(project).await?;
        r.validate(project)?;
        self.io.check().await?;
        Ok(self.history.progress(r, None))
    }
    async fn apply(&mut self, c: Command) -> Result<SetupProgress, NativeError> {
        self.io.check().await?;
        if matches!(c.mutation, Mutation::Begin { .. })
            && self
                .history
                .index(&c.project)
                .is_some_and(|i| self.history.data.records[i].original.is_some())
        {
            return Err(failure(409, "local_setup_already_begun"));
        }
        let n = self.history.add(c)?;
        let k = self.history.data.records[n].commands.len() - 1;
        self.execute(n, k).await
    }
    async fn retry(&mut self, id: &str) -> Result<SetupProgress, NativeError> {
        key(id)?;
        let (n, k) = self
            .history
            .data
            .records
            .iter()
            .enumerate()
            .find_map(|(n, r)| r.commands.iter().position(|c| c.id == id).map(|k| (n, k)))
            .ok_or_else(history_error)?;
        self.execute(n, k).await
    }
    async fn resume_begin(&mut self, project: &str) -> Result<SetupProgress, NativeError> {
        let n = self.history.index(project).ok_or_else(history_error)?;
        let k = self.history.data.records[n]
            .commands
            .iter()
            .rposition(|c| matches!(c.mutation, Mutation::Begin { .. }))
            .ok_or_else(history_error)?;
        self.execute(n, k).await
    }
    async fn execute(&mut self, n: usize, k: usize) -> Result<SetupProgress, NativeError> {
        self.io.check().await?;
        let c = self.history.data.records[n].commands[k].clone();
        if let Outcome::Failed { status, code } = c.outcome {
            return Err(failure(status, &code));
        }
        if c.attempted {
            let recovered = match self.io.recover(&c).await {
                Err(e) if matches!(c.mutation, Mutation::Assign { .. })
                    && e.code == "project_administrator_assignment_retired"
                    && c.outcome == Outcome::Pending => {
                    // The current server checked the durable receipt under its
                    // command lock and confirmed absence of this retired action.
                    self.io.check().await?;
                    self.history.data.records[n].commands[k].outcome = Outcome::Failed {
                        status: e.status, code: e.code.clone(),
                    };
                    self.history.save()?;
                    return Err(e);
                }
                result => result?,
            };
            if let Some(r) = recovered {
                self.accept(n, k, &r)?;
                self.io.check().await?;
                return Ok(self.history.progress(r, None));
            }
            if c.outcome == Outcome::Succeeded {
                return Err(failure(409, "setup_receipt_missing"));
            }
        }
        if matches!(
            c.mutation,
            Mutation::Begin {
                allocation_id: None,
                ..
            }
        ) {
            // Legacy pre-begin journals can recover a recorded original result,
            // but cannot select today's allocation or issue a new begin POST.
            let code = "project_setup_allocation_required";
            if !c.attempted {
                self.history.data.records[n].commands[k].outcome = Outcome::Failed {
                    status: 409,
                    code: code.into(),
                };
                self.history.save()?;
            }
            return Err(failure(409, code));
        }
        let r = self.io.read(&c.project).await?;
        r.validate(&c.project)?;
        let ready = match &c.mutation {
            Mutation::Begin { .. } => r.can_begin,
            Mutation::Assign { .. } => r.can_assign_administrator,
            Mutation::Cancel { setup_id, .. } => {
                r.can_cancel && r.setup.as_ref().is_some_and(|s| &s.id == setup_id)
            }
            Mutation::Rebase { setup_id, .. } => {
                r.can_rebase && r.setup.as_ref().is_some_and(|s| &s.id == setup_id)
            }
            Mutation::Confirm { setup_id, .. } => {
                r.can_confirm && r.setup.as_ref().is_some_and(|s| &s.id == setup_id)
            }
        };
        if !ready {
            let code = r
                .blocking_reason
                .as_deref()
                .unwrap_or("project_setup_not_ready");
            if !c.attempted {
                self.history.data.records[n].commands[k].outcome = Outcome::Failed {
                    status: 409,
                    code: code.into(),
                };
                self.history.save()?;
            }
            return Err(failure(409, code));
        }
        self.io.check().await?;
        self.history.data.records[n].commands[k].attempted = true;
        self.history.save()?;
        match self.io.command(&c).await {
            Ok(r) => {
                self.accept(n, k, &r)?;
                self.io.check().await?;
                Ok(self.history.progress(r, None))
            }
            Err(e) => {
                if !c.attempted && c.no_effect(&e.code) {
                    self.history.data.records[n].commands[k].outcome = Outcome::Failed {
                        status: e.status,
                        code: e.code.clone(),
                    };
                    self.history.save()?;
                }
                Err(e)
            }
        }
    }
    fn accept(&mut self, n: usize, k: usize, r: &ProjectReadiness) -> Result<(), NativeError> {
        let c = &self.history.data.records[n].commands[k];
        r.validate(&c.project)?;
        if let Some(original) = &self.history.data.records[n].original
            && r.setup.as_ref().is_none_or(|s| !original.same_original(s))
        {
            return Err(failure(409, "project_setup_changed"));
        }
        if let Mutation::Begin {
            allocation_id,
            expected_project_revision,
            expected_authority_revision,
            ..
        } = &c.mutation
        {
            let s = r.setup.as_ref().ok_or_else(invalid)?;
            if s.mode != "create"
                || s.manager_mxid != self.io.owner()
                || s.project_revision != *expected_project_revision
                || s.authority_revision != *expected_authority_revision
                || allocation_id
                    .as_ref()
                    .is_some_and(|id| id != &s.allocation_id)
            {
                return Err(invalid());
            }
            if self.history.data.records[n].original.is_none() {
                // A latest historical projection with already-confirmed rooms
                // cannot initialize never-issued creation stages.
                if s.space_id.is_some() || s.room_id.is_some() || s.domain_project_id.is_some() {
                    return Err(history_error());
                }
                let record = &mut self.history.data.records[n];
                record.original = Some(s.clone());
                record.space = Some(Stage::new());
                record.room = Some(Stage::new());
            }
        }
        self.history.data.records[n].commands[k].outcome = Outcome::Succeeded;
        self.history.save()
    }
    async fn ready(&self, n: usize) -> Result<ProjectReadiness, NativeError> {
        let record = &self.history.data.records[n];
        let old = record.original.as_ref().ok_or_else(history_error)?;
        let r = self.io.read(&record.project).await?;
        r.validate(&record.project)?;
        let s = r.setup.as_ref().ok_or_else(history_error)?;
        if !s.same_original(old) || s.manager_mxid != self.io.owner() {
            return Err(failure(409, "project_setup_changed"));
        }
        // Confirmed server IDs are observations, never new creation authority.
        // A restored local never-issued snapshot cannot create another room.
        for (remote, local) in [(&s.space_id, &record.space), (&s.room_id, &record.room)] {
            if let Some(remote) = remote {
                let local = local.as_ref().ok_or_else(history_error)?;
                if matches!(local.state, StageState::NotIssued) {
                    return Err(history_error());
                }
                if local.room_id.as_ref().is_some_and(|id| id != remote) {
                    return Err(failure(409, "project_matrix_link_immutable"));
                }
            }
        }
        if !r.can_confirm
            || !r.capacity_ready
            || s.state == "cancelled"
            || s.current_binding.is_none()
        {
            return Err(failure(
                409,
                r.blocking_reason
                    .as_deref()
                    .unwrap_or("project_setup_not_ready"),
            ));
        }
        let b = s.current_binding.as_ref().unwrap();
        if r.administrator
            .as_ref()
            .is_none_or(|a| a.mxid != b.administrator_mxid || a.revision != b.authority_revision)
        {
            return Err(failure(409, "project_administrator_changed"));
        }
        self.io.check().await?;
        Ok(r)
    }
    async fn continue_setup(
        &mut self,
        project: &str,
        candidates: SetupCandidates,
    ) -> Result<SetupProgress, NativeError> {
        key(project)?;
        for id in [&candidates.space_id, &candidates.room_id]
            .into_iter()
            .flatten()
        {
            room_id(id)?;
        }
        let n = self.history.index(project).ok_or_else(history_error)?;
        if self.history.data.records[n].original.is_none() {
            return Err(history_error());
        }
        if self.history.data.records[n]
            .commands
            .iter()
            .any(|c| c.outcome == Outcome::Pending)
        {
            return Err(failure(409, "local_setup_command_pending"));
        }
        self.ready(n).await?;
        let Some(space) = self.stage(n, true, candidates.space_id, None).await? else {
            return self
                .blocked(project, "space_creation_outcome_unknown")
                .await;
        };
        let Some(room) = self
            .stage(n, false, candidates.room_id, Some(space.clone()))
            .await?
        else {
            return self.blocked(project, "room_creation_outcome_unknown").await;
        };
        if space == room {
            return Err(invalid());
        }
        let server = self
            .io
            .owner()
            .split_once(':')
            .ok_or_else(invalid)?
            .1
            .to_owned();
        for parent in [false, true] {
            self.ready(n).await?;
            let id = if parent { &room } else { &space };
            let key = if parent { &space } else { &room };
            let kind = if parent {
                "m.space.parent"
            } else {
                "m.space.child"
            };
            let state = self.io.matrix(matrix::Operation::State(id.clone())).await?;
            let payload = self.payload(n, !parent)?;
            matrix::prove(&state, &payload)?;
            match matrix::event(&state, kind, key)? {
                Some(e) if e["content"] == matrix::link_content(parent, &server) => {}
                Some(_) => return Err(failure(409, "project_matrix_link_conflict")),
                None => {
                    self.ready(n).await?;
                    self.io
                        .matrix(matrix::Operation::Link {
                            space: space.clone(),
                            room: room.clone(),
                            parent,
                            server: server.clone(),
                        })
                        .await?;
                    self.ready(n).await?;
                }
            }
        }
        let r = self.ready(n).await?;
        let binding = r
            .setup
            .as_ref()
            .unwrap()
            .current_binding
            .as_ref()
            .unwrap()
            .clone();
        let mut admin_joined = true;
        for (id, is_space) in [(&space, true), (&room, false)] {
            let current = self.ready(n).await?;
            if current.setup.as_ref().unwrap().current_binding.as_ref() != Some(&binding) {
                return Err(failure(409, "project_administrator_changed"));
            }
            let state = self.io.matrix(matrix::Operation::State(id.clone())).await?;
            let payload = self.payload(n, is_space)?;
            matrix::prove(&state, &payload)?;
            let membership = matrix::member(&state, &binding.administrator_mxid)?;
            if membership.as_deref() != Some("join") {
                admin_joined = false;
            }
            if matches!(membership.as_deref(), Some("join" | "invite")) {
                continue;
            }
            if self.history.data.records[n]
                .invites
                .iter()
                .any(|i| i.room == *id && i.admin == binding.administrator_mxid)
            {
                return self
                    .blocked(project, "administrator_invitation_requires_review")
                    .await;
            }
            if self.history.data.records[n].invites.len() >= 32 {
                return Err(failure(409, "local_setup_history_full"));
            }
            self.ready(n).await?;
            self.history.data.records[n].invites.push(Invite {
                binding_revision: binding.revision,
                admin: binding.administrator_mxid.clone(),
                room: id.clone(),
            });
            self.history.save()?;
            self.io
                .matrix(matrix::Operation::Invite {
                    room: id.clone(),
                    user: binding.administrator_mxid.clone(),
                })
                .await?;
            self.ready(n).await?;
        }
        // The typed central Confirm command accepts the configured service's
        // exact invitations before verifying membership. Waiting for its join
        // here deadlocks a fresh setup: no generic binding exists yet.
        if !admin_joined {
            return self.blocked(project, "project_membership_pending").await;
        }
        let r = self.ready(n).await?;
        let s = r.setup.as_ref().unwrap();
        if r.matrix_ready {
            return Ok(self.history.progress(r, None));
        }
        self.apply(Command::new(
            project.into(),
            Mutation::Confirm {
                setup_id: s.id.clone(),
                expected_revision: s.revision,
                space_id: space,
                room_id: room,
            },
        )?)
        .await
    }
    async fn blocked(&self, project: &str, code: &str) -> Result<SetupProgress, NativeError> {
        let r = self.io.read(project).await?;
        r.validate(project)?;
        self.io.check().await?;
        Ok(self.history.progress(r, Some(code.into())))
    }
    fn payload(&self, n: usize, space: bool) -> Result<matrix::Create, NativeError> {
        let r = &self.history.data.records[n];
        (if space { &r.space } else { &r.room })
            .as_ref()
            .and_then(|s| s.payload.clone())
            .ok_or_else(history_error)
    }
    async fn stage(
        &mut self,
        n: usize,
        space: bool,
        candidate: Option<String>,
        parent: Option<String>,
    ) -> Result<Option<String>, NativeError> {
        let current = self.ready(n).await?;
        let setup = current.setup.as_ref().ok_or_else(history_error)?;
        let remote = if space {
            &setup.space_id
        } else {
            &setup.room_id
        };
        let candidate = match (candidate, remote) {
            (Some(id), Some(remote)) if &id != remote => {
                return Err(failure(409, "project_matrix_link_immutable"));
            }
            (None, Some(remote)) => Some(remote.clone()),
            (candidate, _) => candidate,
        };
        let stage = if space {
            self.history.data.records[n].space.clone()
        } else {
            self.history.data.records[n].room.clone()
        }
        .ok_or_else(history_error)?;
        if stage
            .room_id
            .as_ref()
            .zip(candidate.as_ref())
            .is_some_and(|(a, b)| a != b)
        {
            return Err(failure(409, "project_matrix_link_immutable"));
        }
        let payload = if let Some(p) = stage.payload.clone() {
            if p.parent != parent || p.space != space {
                return Err(history_error());
            }
            p
        } else {
            if !matches!(stage.state, StageState::NotIssued) {
                return Err(history_error());
            }
            let v = self.io.matrix(matrix::Operation::Discovery).await?;
            let server = self.io.owner().split_once(':').ok_or_else(invalid)?.1;
            let service = format!("@_hagency_service:{server}");
            if v["serverName"] != server || v["serviceMxid"] != service {
                return Err(invalid());
            }
            matrix::Create {
                setup: self.history.data.records[n]
                    .original
                    .clone()
                    .ok_or_else(history_error)?,
                space,
                parent,
                service,
            }
        };
        let mut stage = stage;
        stage.payload = Some(payload.clone());
        if matches!(stage.state, StageState::NotIssued) {
            if candidate.is_some() {
                return Err(failure(409, "setup_stage_not_submitted"));
            }
            self.ready(n).await?;
            stage.state = StageState::Submitted;
            self.put_stage(n, space, stage.clone())?;
            // This is the only create POST. Any error leaves Submitted durably.
            let response = self
                .io
                .matrix(matrix::Operation::Create(Box::new(payload.clone())))
                .await;
            if let Ok(v) = &response
                && let Some(id) = v["room_id"].as_str().filter(|id| room_id(id).is_ok())
            {
                stage.state = StageState::Known;
                stage.room_id = Some(id.into());
                self.put_stage(n, space, stage.clone())?;
            }
            self.ready(n).await?;
            if response.is_err() || stage.room_id.is_none() {
                return Ok(None);
            }
        }
        if let Some(id) = stage.room_id.clone().or(candidate) {
            let state = self.io.matrix(matrix::Operation::State(id.clone())).await?;
            matrix::prove(&state, &payload)?;
            self.ready(n).await?;
            stage.state = StageState::Known;
            stage.room_id = Some(id.clone());
            self.put_stage(n, space, stage)?;
            return Ok(Some(id));
        }
        let joined = self.io.matrix(matrix::Operation::Joined).await?;
        let ids = joined["joined_rooms"].as_array().ok_or_else(invalid)?;
        if ids.len() > 64 {
            return Err(failure(409, "setup_recovery_candidate_required"));
        }
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut found = None;
        for id in ids {
            if std::time::Instant::now() >= end {
                return Err(failure(409, "setup_recovery_candidate_required"));
            }
            let id = id.as_str().ok_or_else(invalid)?;
            room_id(id)?;
            let state = self.io.matrix(matrix::Operation::State(id.into())).await?;
            if matrix::prove(&state, &payload).is_ok() {
                if found.is_some() {
                    return Err(failure(409, "project_matrix_proof_conflict"));
                }
                found = Some(id.to_owned());
            }
        }
        self.ready(n).await?;
        if let Some(id) = &found {
            stage.state = StageState::Known;
            stage.room_id = Some(id.clone());
            self.put_stage(n, space, stage)?;
        }
        Ok(found)
    }
    fn put_stage(&mut self, n: usize, space: bool, stage: Stage) -> Result<(), NativeError> {
        if space {
            self.history.data.records[n].space = Some(stage);
        } else {
            self.history.data.records[n].room = Some(stage);
        }
        self.history.save()
    }
}
