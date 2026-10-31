#[path = "control.rs"]
mod control;
pub use control::{ApprovalControlPolicy, ControlUpdate, PreparedApproval, PreparedUpdate};

use super::state::{State, scope};
use super::{
    Error, InterruptDisposition, MAX_DEFERRED, MAX_DEFERRED_BYTES, MAX_TEXT_BYTES, Outcome, Phase,
    ResumeThreadId, Settings, Update,
};
use crate::codex::{ACKNOWLEDGEMENT_MS, Event, MAX_REQUEST_MS, RequestId, TurnScope, transport};
use serde_json::Value;
use std::collections::{BTreeSet, VecDeque};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::{Instant, timeout_at};

/// A turn that keeps asking for what the adapter refuses is ended by the
/// refusal itself once this many requests have been declined.
const MAX_POLICY_DECLINES: usize = 8;

/// One disposable upstream turn. The host supplies already-owned streams and
/// separately holds all process, Hagency task, approval and lease authority.
pub struct SessionDriver<R, W, E> {
    wire: transport::Driver<R, W, E>,
    settings: Settings,
    state: State,
    response_timeout_ms: u64,
    /// The operation ceiling the session was granted (`transport::Limits`
    /// `lifetime_ms`). The acknowledgement budget is `min` of it and
    /// `ACKNOWLEDGEMENT_MS`, so a startup RPC can never outlive its operation.
    lifetime_ms: u64,
    deferred: VecDeque<Event>,
    deferred_bytes: usize,
    approvals_enabled: bool,
    control: control::ControlState,
    observation_live: Arc<AtomicBool>,
    observation_sequence: u64,
    observation_kind: super::ObservationKind,
    observation_evidence: super::observation::EvidenceTracker,
    last_server_request: Option<&'static str>,
    /// The refused request's OWN method name, bounded and printable. The shape
    /// label above is a fixed aggregate vocabulary and reads `"unknown"` for a
    /// method native does not know — which is exactly the case that must name
    /// itself. Board #87: a live turn died as `owned_failure: "protocol"` with
    /// no way to tell WHICH server->client request killed it.
    last_server_request_method: Option<String>,
    refused_notification: Option<&'static str>,
    mcp: crate::codex::approval::McpTracker,
    /// Requests this session declined by adapter policy. Their upstream
    /// resolution is not an owner callback and never reaches the coordinator.
    policy_declined: BTreeSet<RequestId>,
    policy_declines: usize,
}

impl<R, W, E> SessionDriver<R, W, E> {
    pub fn new(
        stdout: R,
        stdin: W,
        stderr: E,
        settings: Settings,
        limits: transport::Limits,
        response_timeout_ms: u64,
    ) -> Result<Self, Error> {
        if response_timeout_ms == 0 || response_timeout_ms > MAX_REQUEST_MS {
            return Err(Error::Settings);
        }
        let lifetime_ms = limits.lifetime_ms;
        Ok(Self {
            wire: transport::Driver::new(stdout, stdin, stderr, limits)
                .map_err(Error::Transport)?,
            settings,
            state: State::default(),
            response_timeout_ms,
            lifetime_ms,
            deferred: VecDeque::new(),
            deferred_bytes: 0,
            approvals_enabled: false,
            control: control::ControlState::default(),
            observation_live: Arc::new(AtomicBool::new(true)),
            observation_sequence: 0,
            observation_kind: super::ObservationKind::Ignored,
            observation_evidence: super::observation::EvidenceTracker::default(),
            last_server_request: None,
            last_server_request_method: None,
            refused_notification: None,
            mcp: crate::codex::approval::McpTracker::default(),
            policy_declined: BTreeSet::new(),
            policy_declines: 0,
        })
    }
    pub fn settings(&self) -> &Settings {
        &self.settings
    }
    /// Shared execution chooses a room only after work admission. Keep model,
    /// permissions and activity custody fixed; cwd also drives turn sandbox
    /// roots and the host's approval scope. Never change an active thread.
    pub(crate) fn bind_shared_workspace(&mut self, directory: std::path::PathBuf) -> Result<(), Error> {
        if self.phase() != Phase::Ready || self.wire.is_warm_idle() || self.settings.task_mcp.is_some() {
            return Err(Error::State);
        }
        let root = std::path::Path::new(self.settings.cwd());
        if directory == root || !directory.starts_with(root)
            || directory.canonicalize().ok().as_ref() != Some(&directory) {
            return Err(Error::Scope);
        }
        self.settings.cwd = Settings::new(directory, self.settings.model.clone(), self.settings.effort.clone())?.cwd;
        Ok(())
    }
    /// The acknowledgement budget for a startup RPC (`initialize`,
    /// `thread/start`, `thread/resume`). TS 1:1, and **unconditional**: TS
    /// applies `acknowledgementTimeoutMs` (`?? 60_000`) to these three waits
    /// regardless of whether an MCP server is configured (`router/src/runner.ts`
    /// `:431` default, used at `:807` `Codex initialize` and `:828` `Codex
    /// thread start`). The generic per-request `response_ms` bound is a
    /// different bound and does not apply here.
    ///
    /// The product `response_ms` ceiling (2 s,
    /// `hagency-execution/src/host.rs:58`) sits UNDER Codex's own required-MCP
    /// ceiling (`startup_timeout_sec`, 5 s — `session/task_mcp.rs:50-51`), so a
    /// cold helper lost the race and the live rig died `Transport(Timeout)` at
    /// `stage="thread_start"`, settling the attempt `outcome_unknown` (board
    /// #105). The operation ceiling still wins: `lifetime_ms` bounds every wait
    /// independently, so this never outlives the session it was granted.
    fn acknowledgement_ms(&self) -> u64 {
        ACKNOWLEDGEMENT_MS.min(self.lifetime_ms)
    }
    /// Bind this dispatch's permissions before any thread is started or resumed.
    /// A consumed warm process is still in Ready; active threads cannot change.
    /// Cwd/model/effort and the retained task helper remain the same identities.
    pub fn bind_dispatch_policy(&mut self, settings: &Settings) -> Result<(), Error> {
        if self.phase() != Phase::Ready || self.wire.is_warm_idle() {
            return Err(Error::State);
        }
        if self.settings.cwd != settings.cwd
            || self.settings.model != settings.model
            || self.settings.effort != settings.effort
        {
            return Err(Error::Scope);
        }
        self.settings.read_only = settings.read_only;
        self.settings.unrestricted = settings.unrestricted;
        Ok(())
    }
    /// One fixed Host helper after initialize, never general reconfiguration.
    pub fn bind_task_mcp(&mut self, helper: super::TaskMcp) -> Result<(), Error> {
        if self.phase() != Phase::Ready
            || self.settings.task_mcp.is_some()
            || self.wire.is_warm_idle()
        {
            return Err(Error::State);
        }
        self.settings.task_mcp = Some(helper);
        Ok(())
    }
    /// Opt in only before initialize. Ordinary/active sessions cannot extend IO.
    pub fn reserve_warm_idle(&mut self) -> Result<(), Error> {
        if self.phase() != Phase::New || self.settings.task_mcp.is_some() {
            return Err(Error::State);
        }
        self.wire.reserve_warm_idle().map_err(Error::Transport)
    }
    pub(crate) fn renew_shared_activity(&mut self, until: Instant) -> Result<(), Error> {
        self.wire
            .renew_shared_activity(until)
            .map_err(Error::Transport)
    }
    pub fn enter_warm_idle(&mut self, until: Instant) -> Result<(), Error> {
        if self.phase() != Phase::Ready || self.settings.task_mcp.is_some() {
            return Err(Error::State);
        }
        self.wire.enter_warm_idle(until).map_err(Error::Transport)
    }
    pub(crate) fn attach_shared_activity(
        &mut self,
        clock: std::sync::Arc<dyn Fn() -> Option<Instant> + Send + Sync>,
    ) -> Result<(), Error> {
        self.wire
            .attach_shared_activity(clock)
            .map_err(Error::Transport)
    }
    pub fn consume_warm_idle(
        &mut self,
        limits: transport::Limits,
        response_timeout_ms: u64,
        until: Instant,
    ) -> Result<(), Error> {
        if self.phase() != Phase::Ready
            || self.settings.task_mcp.is_some()
            || response_timeout_ms == 0
            || response_timeout_ms > MAX_REQUEST_MS
            || response_timeout_ms > limits.lifetime_ms
        {
            return Err(Error::State);
        }
        self.wire
            .consume_warm_idle(limits, until)
            .map_err(Error::Transport)?;
        self.response_timeout_ms = response_timeout_ms;
        Ok(())
    }
    /// Attach once the exact turn is Running, before consuming any updates.
    pub fn observation_source(&self) -> Result<super::ObservationSource, Error> {
        if self.phase() != Phase::Running || self.observation_sequence != 0 {
            return Err(Error::State);
        }
        self.bound_source()
    }
    fn bound_source(&self) -> Result<super::ObservationSource, Error> {
        Ok(super::ObservationSource {
            live: self.observation_live.clone(),
            thread: self.thread_id().ok_or(Error::State)?.into(),
            turn: self.turn_id().ok_or(Error::State)?.into(),
        })
    }
    pub fn matches_observation_source(&self, source: &super::ObservationSource) -> bool {
        self.phase() == Phase::Running
            && !source.is_retired()
            && self.bound_source().is_ok_and(|value| &value == source)
    }
    /// Explicit host opt-in after the upstream thread and turn were validated.
    pub fn enable_approvals(&mut self) -> Result<(), Error> {
        if self.phase() != Phase::Running || self.approvals_enabled {
            return Err(Error::State);
        }
        self.approvals_enabled = true;
        Ok(())
    }
    pub fn phase(&self) -> Phase {
        self.state.phase
    }
    pub fn thread_id(&self) -> Option<&str> {
        self.state.thread.as_deref()
    }
    pub fn turn_id(&self) -> Option<&str> {
        self.state.turn.as_deref()
    }
    pub fn outcome(&self) -> Option<&Outcome> {
        self.state.outcome.as_ref()
    }
    pub fn item_count(&self) -> usize {
        self.state.item_count()
    }
    pub fn text_bytes(&self) -> usize {
        self.state.text_bytes()
    }
    pub fn event_count(&self) -> usize {
        self.state.event_count()
    }
    pub fn transport_termination(&self) -> Option<&transport::Termination> {
        self.wire.termination()
    }
    /// Fixed shape label only; no callback params, IDs or runtime text.
    pub fn last_server_request(&self) -> Option<&'static str> {
        self.last_server_request
    }
    /// The refused server request's own method name, bounded and printable.
    /// Diagnostic only (ADR-181 shape): it names WHAT was refused so a live
    /// failure is actionable, and it is never authority.
    pub fn last_server_request_method(&self) -> Option<&str> {
        self.last_server_request_method.as_deref()
    }
    /// Fixed category of the refused notification, never a peer method/ID/text.
    pub fn refused_notification(&self) -> Option<&'static str> {
        self.refused_notification
    }
    /// The provider's own reason for ending the turn, bounded at admission
    /// (board #110). Diagnostic only: TS surfaces the same words
    /// (`router/src/runner.ts:793`), and without them a usage-limit refusal read
    /// as a bare `protocol` fault. Never a path, capability or secret.
    pub fn turn_failure(&self) -> Option<&str> {
        self.state.turn_failure()
    }
    /// Whether the connection still holds this prepared server request. False
    /// once `serverRequest/resolved` was parsed: the one-shot frame's transmit
    /// path is gone, so it must never be re-sent.
    pub fn prepared_admissible(&self, id: &RequestId) -> bool {
        self.wire.prepared_admissible(id)
    }
    /// `(accepted, total)` bytes of the frame in write custody, or `None`.
    /// Read-only projection for the approval turn-end rule; carries no
    /// authority and never influences the write itself.
    pub fn write_progress(&self) -> Option<(usize, usize)> {
        self.wire.write_progress()
    }
    pub fn stderr_snapshot(&self) -> transport::StderrSnapshot {
        self.wire.stderr_snapshot()
    }

    pub fn close(&mut self) {
        if self.state.phase != Phase::Ended {
            self.fail(Error::Cancelled);
        }
        if !matches!(self.outcome(), Some(Outcome::Completed { .. })) {
            self.observation_live.store(false, Ordering::Release);
        }
        self.wire.close();
    }
    fn fail(&mut self, error: Error) {
        self.observation_live.store(false, Ordering::Release);
        self.state.failed(error);
        self.wire.close();
        self.deferred.clear();
        self.deferred_bytes = 0;
    }
    fn buffer(&mut self, event: Event) -> Result<(), Error> {
        let Event::Notification {
            method,
            params: Some(params),
        } = &event
        else {
            return Err(Error::Malformed);
        };
        // A new thread is unbound until its RPC response, so no TURN-scoped
        // notice can be legitimate yet: refuse one that declares a turn.
        // Everything else is a thread-lifecycle or global notice, and a method
        // this build does not know is tolerated by name (board #94) instead of
        // refused. Refusing by method is what killed live dispatches (#89).
        if self.state.phase == Phase::OpeningThread
            && params.get("turnId").is_some_and(|value| !value.is_null())
        {
            tracing::warn!(
                method = method.as_str(),
                "refusing turn-scoped Codex notification while the thread is unbound"
            );
            return Err(Error::Scope);
        }
        if self.state.phase == Phase::OpeningThread
            && matches!(method.as_str(), "hook/started" | "hook/completed")
            && params.get("turnId").is_some_and(|v| !v.is_null())
        {
            return Err(Error::Scope);
        }
        scope(
            method,
            params,
            self.state.thread.as_deref(),
            self.state.turn.as_deref(),
        )?;
        let charge = transport::event_bytes(&event).map_err(Error::Transport)?;
        let total = self
            .deferred_bytes
            .checked_add(charge)
            .filter(|&n| n <= MAX_DEFERRED_BYTES)
            .ok_or(Error::Capacity)?;
        if self.deferred.len() >= MAX_DEFERRED {
            return Err(Error::Capacity);
        }
        self.deferred.push_back(event);
        self.deferred_bytes = total;
        Ok(())
    }
    fn validate_deferred(&self) -> Result<(), Error> {
        for event in &self.deferred {
            let Event::Notification {
                method,
                params: Some(params),
            } = event
            else {
                return Err(Error::Malformed);
            };
            scope(
                method,
                params,
                self.state.thread.as_deref(),
                self.state.turn.as_deref(),
            )?;
        }
        Ok(())
    }
    fn pop(&mut self) -> Result<Option<Event>, Error> {
        let Some(event) = self.deferred.pop_front() else {
            return Ok(None);
        };
        self.deferred_bytes = self
            .deferred_bytes
            .checked_sub(transport::event_bytes(&event).map_err(Error::Transport)?)
            .ok_or(Error::Capacity)?;
        Ok(Some(event))
    }
}
impl<R, W, E> Drop for SessionDriver<R, W, E> {
    fn drop(&mut self) {
        if !matches!(self.outcome(), Some(Outcome::Completed { .. })) {
            self.observation_live.store(false, Ordering::Release);
        }
    }
}

// Dropping any started operation poisons the typed state even when cancellation
// happens between lower-level IO calls. Closing streams does not stop a child.
struct Operation<'a, R, W, E> {
    session: &'a mut SessionDriver<R, W, E>,
    finished: bool,
}
impl<R, W, E> Drop for Operation<'_, R, W, E> {
    fn drop(&mut self) {
        if !self.finished {
            self.session.fail(Error::Cancelled);
        }
    }
}
impl<R, W, E> Operation<'_, R, W, E> {
    fn finish<T>(mut self, result: Result<T, Error>) -> Result<T, Error> {
        if let Err(error) = result.as_ref() {
            self.session.fail(*error);
        }
        self.finished = true;
        result
    }
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin, E: AsyncRead + Unpin> SessionDriver<R, W, E> {
    pub async fn initialize(&mut self) -> Result<(), Error> {
        if self.phase() != Phase::New {
            return Err(Error::State);
        }
        let operation = Operation {
            session: self,
            finished: false,
        };
        let result = operation.session.initialize_inner().await;
        operation.finish(result)
    }
    async fn initialize_inner(&mut self) -> Result<(), Error> {
        self.state.phase = Phase::Initializing;
        self.wire
            .send(transport::Command::Initialize {
                client_version: env!("CARGO_PKG_VERSION").into(),
                // Startup RPC: TS bounds `Codex initialize` by the same 60 s
                // acknowledgement budget (`router/src/runner.ts:807`), not the
                // generic per-request bound (board #105).
                response_timeout_ms: self.acknowledgement_ms(),
            })
            .await
            .map_err(Error::Transport)?;
        loop {
            match self.receive().await? {
                Event::Initialized { .. } => break,
                // Board #94: a startup notification is never fatal, whatever its
                // method. This used to be a per-method allowlist, and every
                // method Codex added across versions (account/updated, board #89)
                // killed the dispatch. `state.notification` validates the ones it
                // models and logs the rest by name.
                Event::Notification {
                    method,
                    params: Some(params),
                } => {
                    self.state.notification(&method, &params)?;
                }
                Event::Notification { method, .. } => {
                    tracing::debug!(
                        method = method.as_str(),
                        "ignoring parameterless Codex notification during startup"
                    );
                }
                // A request during startup has no owner to answer it: answer it
                // ourselves and let startup continue.
                Event::ServerRequest { id, .. } => self.answer_request(id).await?,
                _ => return Err(Error::Scope),
            }
        }
        self.wire
            .send(transport::Command::Initialized)
            .await
            .map_err(Error::Transport)?;
        self.state.phase = Phase::Ready;
        Ok(())
    }

    /// Three fixed reads only. No caller method, parameters, thread or prompt.
    /// Startup server requests invalidate qualification even when rejected.
    pub async fn qualification_read(
        &mut self,
        read: super::QualificationRead,
    ) -> Result<Value, Error> {
        // These closed metadata reads run before any thread/model turn. Tool
        // sessions must inspect their configuration and account too; their
        // execution policy is verified separately on the same actual process.
        if self.phase() != Phase::Ready
            || self.settings.task_mcp.is_some()
            || self.last_server_request.is_some()
        {
            return Err(Error::Policy);
        }
        let operation = Operation {
            session: self,
            finished: false,
        };
        let (method, params) = read.request();
        let result = operation
            .session
            .call_bounded(method, params, operation.session.response_timeout_ms)
            .await;
        let result = if operation.session.last_server_request.is_some() {
            Err(Error::UnsupportedRequest)
        } else {
            result
        };
        operation.finish(result)
    }
    pub async fn start_thread(&mut self) -> Result<String, Error> {
        self.open_thread(None).await
    }
    /// Explicit host-recorded upstream identity only; this never authorizes
    /// automatic resume/replay of a Hagency dispatch or an unknown prior turn.
    pub async fn resume_thread(&mut self, id: ResumeThreadId) -> Result<String, Error> {
        self.open_thread(Some(id)).await
    }
    async fn open_thread(&mut self, resume: Option<ResumeThreadId>) -> Result<String, Error> {
        if self.phase() != Phase::Ready || self.wire.is_warm_idle() {
            return Err(Error::State);
        }
        let operation = Operation {
            session: self,
            finished: false,
        };
        let result = operation
            .session
            .open_thread_inner(resume.as_ref().map(|id| id.0.as_str()))
            .await;
        operation.finish(result)
    }
    async fn open_thread_inner(&mut self, resume: Option<&str>) -> Result<String, Error> {
        self.state.phase = Phase::OpeningThread;
        // For resume, reject a substituted notification even before its response.
        self.state.thread = resume.map(str::to_owned);
        let method = if resume.is_some() {
            "thread/resume"
        } else {
            "thread/start"
        };
        let result = self
            // Startup RPC: Codex answers `thread/start`/`thread/resume` only
            // after every REQUIRED MCP server in the thread config has
            // handshaked (`task_mcp.rs:51`, `startup_timeout_sec`), so it uses
            // the startup floor rather than the generic per-request bound.
            .startup_call(method, self.settings.thread_request(resume))
            .await?;
        let id = self.state.observe_thread(&result, &self.settings, resume)?;
        self.validate_deferred()?;
        while let Some(event) = self.pop()? {
            if let Event::Notification {
                method,
                params: Some(params),
            } = event
            {
                self.state.notification(&method, &params)?;
            }
        }
        Ok(id)
    }

    pub async fn start_turn(&mut self, input: String) -> Result<String, Error> {
        if self.phase() != Phase::ThreadReady {
            return Err(Error::State);
        }
        if input.is_empty() || input.len() > MAX_TEXT_BYTES {
            return Err(Error::Settings);
        }
        let operation = Operation {
            session: self,
            finished: false,
        };
        let result = operation.session.start_turn_inner(input).await;
        operation.finish(result)
    }
    async fn start_turn_inner(&mut self, input: String) -> Result<String, Error> {
        self.state.phase = Phase::StartingTurn;
        let thread = self.state.thread.as_deref().ok_or(Error::State)?;
        let result = self
            .call("turn/start", self.settings.turn_request(thread, input))
            .await?;
        let id = self.state.observe_turn(&result)?;
        self.validate_deferred()?;
        Ok(id)
    }

    pub async fn interrupt(&mut self) -> Result<InterruptDisposition, Error> {
        if self.phase() != Phase::Running || self.state.interrupt_sent {
            return Err(Error::State);
        }
        let operation = Operation {
            session: self,
            finished: false,
        };
        let result = operation.session.interrupt_inner().await;
        operation.finish(result)
    }
    async fn interrupt_inner(&mut self) -> Result<InterruptDisposition, Error> {
        self.state.interrupt_sent = true;
        let scope = TurnScope::new(
            self.state.thread.clone().ok_or(Error::State)?,
            self.state.turn.clone().ok_or(Error::State)?,
        )
        .map_err(|_| Error::Scope)?;
        self.wire
            .send(transport::Command::Interrupt {
                scope: scope.clone(),
                response_timeout_ms: self.response_timeout_ms,
            })
            .await
            .map_err(Error::Transport)?;
        loop {
            match self.receive().await? {
                Event::InterruptAcknowledged { scope: observed } if observed == scope => {
                    return Ok(InterruptDisposition::Acknowledged);
                }
                Event::InterruptRejected {
                    scope: observed,
                    error,
                } if observed == scope => {
                    return Ok(InterruptDisposition::Rejected { code: error.code });
                }
                event @ Event::Notification { .. } => self.buffer(event)?,
                _ => return Err(Error::Scope),
            }
        }
    }

    pub async fn respond_approval(
        &mut self,
        response: crate::codex::approval::ApprovalResponse,
    ) -> Result<(), Error> {
        if self.phase() != Phase::Running || !self.approvals_enabled || self.control.enabled() {
            return Err(Error::State);
        }
        if self.thread_id() != Some(response.request.thread_id())
            || self.turn_id() != Some(response.request.turn_id())
            || response
                .request
                .mcp_item()
                .is_some_and(|id| !self.mcp.active(id))
        {
            return Err(Error::Scope);
        }
        let operation = Operation {
            session: self,
            finished: false,
        };
        let result = operation
            .session
            .wire
            .send(transport::Command::RespondApproval { response })
            .await
            .map(|_| ())
            .map_err(Error::Transport);
        operation.finish(result)
    }

    pub async fn next_update(&mut self) -> Result<Update, Error> {
        if self.phase() != Phase::Running {
            return Err(Error::State);
        }
        let operation = Operation {
            session: self,
            finished: false,
        };
        let mut result = operation.session.update_inner().await;
        if result.is_ok()
            && let Err(error) = operation.session.advance_observation()
        {
            result = Err(error);
        }
        operation.finish(result)
    }
    pub async fn next_observed_update(&mut self) -> Result<(Update, super::Observation), Error> {
        let update = self.next_update().await?;
        let observation = super::Observation {
            source: self.bound_source()?,
            sequence: self.observation_sequence,
            kind: self.observation_kind.clone(),
        };
        Ok((update, observation))
    }
    async fn update_inner(&mut self) -> Result<Update, Error> {
        self.wire.ensure_live().map_err(Error::Transport)?;
        let event = match self.pop()? {
            Some(event) => event,
            None => self.receive().await?,
        };
        self.accept_update(event).await
    }
    async fn accept_update(&mut self, event: Event) -> Result<Update, Error> {
        let event = match event {
            Event::ServerRequest {
                id,
                method,
                params: Some(params),
            } if self.approvals_enabled => {
                self.last_server_request = Some(server_request_shape(&method));
                let method_name = bounded_method(&method);
                self.last_server_request_method = Some(method_name.clone());
                // The fixed shape label is what status carries: a private runtime
                // string is never projected (see
                // `native_codex_session_outcomes_refused_callbacks_expose_only_fixed_shape_labels`),
                // and unlike `method` it is `Copy`, so it survives `parse`.
                let shape = server_request_shape(&method);
                let elicitation = method == "mcpServer/elicitation/request";
                let original = id.clone();
                let declinable = crate::codex::approval::policy_decline(&method, &params).is_some();
                let parsed = if params.get("threadId").and_then(Value::as_str) != self.thread_id()
                    || params.get("turnId").and_then(Value::as_str) != self.turn_id()
                {
                    Err(Error::Scope)
                } else if elicitation {
                    self.mcp.request(id, params)
                } else {
                    crate::codex::approval::ApprovalRequest::parse(id, method, params)
                };
                let request = match parsed {
                    Ok(request) => request,
                    Err(error) => {
                        // An adapter-refused approval — the unsupported MCP
                        // elicitation FORM — is answered with the family's own
                        // decline and the turn goes on. The retained runner ends
                        // the turn here (`router/src/runner.ts:725-735`); the
                        // operator rule (a bridge-side fault is never terminal)
                        // overrides it, and this is exactly the arm that killed a
                        // live turn as a silent `protocol` when Codex asked an
                        // elicitation. Identity and correlation failures
                        // (`Error::Scope`) are not answered with a decline: Codex
                        // never learns whether that tool ran. The WARN names the
                        // cause the failure previously hid.
                        let adapter_refused = matches!(error, Error::Policy);
                        if adapter_refused
                            && declinable
                            && self.policy_declines < MAX_POLICY_DECLINES
                        {
                            tracing::warn!(
                                shape = shape,
                                method = %method_name,
                                request_id = ?original,
                                "Codex server request refused by the adapter; answering its decline and continuing the turn"
                            );
                            // Refused by the adapter, not by the owner: answer
                            // with the family's own decline and let the turn go
                            // on. The host coordinator never sees this request.
                            self.wire
                                .send(transport::Command::DeclineServerRequest {
                                    id: original.clone(),
                                })
                                .await
                                .map_err(Error::Transport)?;
                            self.policy_declines += 1;
                            self.policy_declined.insert(original);
                            self.observation_kind = super::ObservationKind::Ignored;
                            return Ok(Update::Notice);
                        }
                        tracing::warn!(
                            shape = shape,
                            method = %method_name,
                            request_id = ?original,
                            "unsupported Codex server request; the turn cannot continue"
                        );
                        // A method this build does not know at all is answered
                        // with the wire's own refusal and the turn goes on.
                        // Board #94: an unknown request must never kill the
                        // turn. TS sends the same -32601 and the operator rule
                        // drops only its termination (`router/src/runner.ts:715,726`).
                        if matches!(error, Error::UnsupportedRequest) {
                            self.wire
                                .send(transport::Command::RejectServerRequest { id: original })
                                .await
                                .map_err(Error::Transport)?;
                            self.observation_kind = super::ObservationKind::Ignored;
                            return Ok(Update::Notice);
                        }
                        // A failure this adapter cannot answer with a decline
                        // still gets the wire's own "no" for an elicitation —
                        // the retained runner cancels it (`router/src/runner.ts:727-728`);
                        // other families keep their protocol error response.
                        if elicitation {
                            self.wire
                                .send(transport::Command::RejectServerRequest { id: original })
                                .await
                                .map_err(Error::Transport)?;
                        }
                        return Err(error);
                    }
                };
                if self.thread_id() != Some(request.thread_id())
                    || self.turn_id() != Some(request.turn_id())
                {
                    return Err(Error::Scope);
                }
                // Patch and permission requests can precede item/started in
                // Codex 0.153.4. Bind their exact callback item without inventing
                // an active timeline item or host session identity.
                self.control.admit(&self.wire, request.id())?;
                self.observation_kind = super::ObservationKind::Ignored;
                return Ok(Update::Approval(request));
            }
            // No owner can take this request — approvals are off, or the frame
            // carried no params. Board #94 tolerates a method this build does
            // not know AT ALL: it is answered and the turn goes on (TS sends the
            // same `-32601`, `router/src/runner.ts:715,726`; the operator rule
            // drops only its termination). A RECOGNIZED request is not protocol
            // noise — it is a real product outcome this session cannot grant, so
            // it keeps the named refusal the operation maps to
            // `Failure::UnsupportedApproval`. TS drives every recognized
            // approval to an owner and fails the turn when it cannot
            // (`runner.ts:713-722,733`); it never silently continues.
            Event::ServerRequest { id, method, .. } => {
                self.last_server_request = Some(server_request_shape(&method));
                self.last_server_request_method = Some(bounded_method(&method));
                if server_request_shape(&method) == "unknown" {
                    self.answer_request(id).await?;
                    return Ok(Update::Notice);
                }
                return self.unsupported(id).await;
            }
            event => event,
        };
        let Event::Notification {
            method,
            params: Some(params),
        } = event
        else {
            return Err(Error::Scope);
        };
        let mut update = self.state.notification(&method, &params).inspect_err(|_| {
            self.refused_notification = Some(notification_shape(&method, &params));
        })?;
        self.mcp.observe(&method, &params)?;
        if self.approvals_enabled && method == "serverRequest/resolved" {
            let id =
                serde_json::from_value(params.get("requestId").ok_or(Error::Malformed)?.clone())
                    .map_err(|_| Error::Malformed)?;
            if !self.policy_declined.remove(&id) {
                self.control.resolve(&id);
                update = Update::ApprovalResolved { id };
            }
        }
        if self.phase() == Phase::Ended {
            self.drain_terminal().await?;
            // Shared text remains one-shot (the codec stays Ended), but its
            // actual process must remain owned until the central reply settles.
            if !self.wire.has_live_shared_activity() {
                self.wire.close();
            }
        }
        self.observation_kind = if method == "thread/tokenUsage/updated" {
            super::ObservationKind::Usage(super::usage::project(&params))
        } else {
            self.observation_evidence
                .project(&update, &params, self.state.outcome.as_ref())
        };
        Ok(update)
    }

    async fn drain_terminal(&mut self) -> Result<(), Error> {
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(self.response_timeout_ms))
            .ok_or(Error::Transport(transport::Error::Timeout))?;
        loop {
            // Finish an already-started suffix frame, but never start a read
            // when the received snapshot is empty. Fragmentation alone must not
            // make a normal completed + idle stream fail. The whole drain has
            // one absolute deadline, in addition to transport deadlines.
            tokio::task::yield_now().await;
            self.wire.ensure_live().map_err(Error::Transport)?;
            if Instant::now() >= deadline {
                return Err(Error::Transport(transport::Error::Timeout));
            }
            let mut event = match self.pop()? {
                Some(event) => Some(event),
                None => self.wire.buffered_event().map_err(Error::Transport)?,
            };
            if event.is_none() && self.wire.has_partial_frame() {
                event = Some(
                    timeout_at(deadline, self.receive())
                        .await
                        .map_err(|_| Error::Transport(transport::Error::Timeout))??,
                );
            }
            match event {
                Some(Event::Notification {
                    method,
                    params: Some(params),
                }) => self.state.terminal_suffix(&method, &params)?,
                // Same deferral as the post-loop check below (F1): with
                // approvals enabled a request arriving DURING the drain is
                // the host coordinator's armed callback, not an unowned
                // violation, and erroring here would pre-empt the ADR-046
                // turn-end rule through accept_update's `?` before the
                // TurnEnded update ever reaches it. The runtime's own
                // receive() guard (see below) is the authority for this
                // condition; the drain previously enforced it only after
                // the loop, so a mid-drain arrival still collapsed every
                // turn-end arm into `UnsupportedRequest`.
                Some(Event::ServerRequest { id, .. }) if !self.approvals_enabled => {
                    self.answer_request(id).await?;
                }
                Some(Event::ServerRequest { .. }) => {
                    // The deferral itself: the event is consumed here (it
                    // was already popped from the wire), but the request
                    // stays pending in the connection's server-pending map,
                    // which is exactly the state the post-loop check below
                    // and the ADR-046 turn-end rule classify. Sending any
                    // response — a reject — from the terminal drain would
                    // answer the host's own armed callback.
                }
                Some(_) => return Err(Error::Scope),
                None => break,
            }
        }
        // ADR-046, "the terminal drain must not settle an armed callback"
        // (amended this commit): a pending server request at the terminal
        // drain is a protocol violation only when nothing owns it. With
        // approvals enabled, these requests are the host coordinator's armed
        // approval callbacks — the peer's turn ended before they were
        // answered, which is precisely the state the execution host's
        // ADR-046 turn-end rule classifies (quiet, unknown, or named
        // refusal) from the termination snapshot the `close()` that follows
        // preserves. Erroring here instead would pre-empt that rule with
        // `UnsupportedRequest` and collapse every arm into a protocol fault
        // (the macOS shape: `Protocol` where the quiet family or
        // `SettlementUnknown` is owed).
        if self.wire.pending_server_requests() > 0 && !self.approvals_enabled {
            return Err(Error::UnsupportedRequest);
        }
        Ok(())
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value, Error> {
        self.call_bounded(method, params, self.response_timeout_ms)
            .await
    }
    /// A startup RPC (`initialize`, `thread/start`, `thread/resume`) under the
    /// acknowledgement budget instead of the generic per-request bound. See
    /// `acknowledgement_ms`.
    async fn startup_call(&mut self, method: &str, params: Value) -> Result<Value, Error> {
        self.call_bounded(method, params, self.acknowledgement_ms())
            .await
    }
    async fn call_bounded(
        &mut self,
        method: &str,
        params: Value,
        response_timeout_ms: u64,
    ) -> Result<Value, Error> {
        let written = self
            .wire
            .send(transport::Command::Request {
                method: method.into(),
                params,
                response_timeout_ms,
            })
            .await
            .map_err(Error::Transport)?;
        let expected = written.request_id.ok_or(Error::State)?;
        loop {
            match self.receive().await? {
                Event::Response {
                    id,
                    method: observed,
                    result,
                } if id == expected && observed == method => {
                    return result.map_err(|error| Error::Rejected(error.code));
                }
                event @ Event::Notification { .. } => self.buffer(event)?,
                // A request that races this call has no owner yet (no turn is
                // running), so answer it and keep waiting for our own response.
                Event::ServerRequest { id, .. } => self.answer_request(id).await?,
                _ => return Err(Error::Scope),
            }
        }
    }
    async fn receive(&mut self) -> Result<Event, Error> {
        let event = self.wire.next_event().await.map_err(Error::Transport)?;
        if let Event::ServerRequest { method, .. } = &event {
            self.last_server_request = Some(server_request_shape(method));
            self.last_server_request_method = Some(bounded_method(method));
        }
        Ok(event)
    }
    /// Answer a server->client request this session cannot hand to an owner with
    /// the response the retained runner sends — the JSON-RPC `-32601` error, or
    /// the MCP elicitation's own `cancel` — and keep going. Board #94: an
    /// unknown request must never kill the turn. TS answers and then terminates
    /// (`router/src/runner.ts:709-736`); the operator rule (a bridge-side fault
    /// is never terminal) keeps only the answer.
    async fn answer_request(&mut self, id: RequestId) -> Result<(), Error> {
        tracing::debug!(
            request_id = ?id,
            "answering a Codex server request no owner can take; continuing"
        );
        self.wire
            .send(transport::Command::RejectServerRequest { id })
            .await
            .map_err(Error::Transport)?;
        Ok(())
    }

    /// Answer a RECOGNIZED server->client request that no owner can take with
    /// the wire's own refusal, then fail the session with the named
    /// `UnsupportedRequest` (the operation maps it to
    /// `Failure::UnsupportedApproval`). Board #94 tolerates only a method this
    /// build does not know at all; a recognized approval is a real product
    /// outcome, and TS fails the turn rather than silently continuing
    /// (`router/src/runner.ts:713-722,733`).
    async fn unsupported(&mut self, id: RequestId) -> Result<Update, Error> {
        self.wire
            .send(transport::Command::RejectServerRequest { id })
            .await
            .map_err(Error::Transport)?;
        Err(Error::UnsupportedRequest)
    }
}

/// The refused request's own method name, bounded so a hostile peer cannot stuff
/// the failure detail. Board #87: the fixed shape label reads `"unknown"` for
/// exactly the method that killed a live turn, so the cause must name itself.
/// Diagnostic only (ADR-181): never authority.
fn bounded_method(method: &str) -> String {
    method.chars().take(255).collect()
}

fn server_request_shape(method: &str) -> &'static str {
    match method {
        "item/commandExecution/requestApproval" => "command_approval",
        "item/fileChange/requestApproval" => "file_approval",
        "item/permissions/requestApproval" => "permissions_approval",
        "item/tool/requestUserInput" => "tool_user_input",
        "mcpServer/elicitation/request" => "mcp_elicitation",
        "item/tool/call" => "dynamic_tool_call",
        "account/chatgptAuthTokens/refresh" => "auth_refresh",
        "attestation/generate" => "attestation",
        "execCommandApproval" | "applyPatchApproval" => "legacy_approval",
        _ => "unknown",
    }
}

fn notification_shape(method: &str, params: &Value) -> &'static str {
    match method {
        "item/started" | "item/completed" => match params["item"]["type"].as_str() {
            Some("commandExecution") => "command_item",
            Some("fileChange") => "file_item",
            Some("userMessage") => "user_item",
            Some("agentMessage") => "agent_item",
            Some("reasoning") => "reasoning_item",
            Some("mcpToolCall") => "mcp_item",
            _ => "other_item",
        },
        "thread/status/changed" => "thread_status",
        "turn/started" => "turn_started",
        "turn/completed" => "turn_completed",
        "mcpServer/startupStatus/updated" => "mcp_startup",
        "remoteControl/status/changed" => "remote_control",
        "account/rateLimits/updated" => "account_limits",
        "account/updated" => "account",
        "thread/tokenUsage/updated" => "turn_usage",
        "item/commandExecution/terminalInteraction" => "terminal_interaction",
        "item/fileChange/patchUpdated" => "patch_updated",
        // Fixed diagnostic labels only. Recognition here does not admit these
        // notifications, widen scope, or expose private upstream payloads.
        "thread/settings/updated" => "thread_settings",
        "thread/name/updated" => "thread_name",
        "thread/goal/updated" | "thread/goal/cleared" => "thread_goal",
        "thread/queue/changed" => "thread_queue",
        "thread/environment/connected" | "thread/environment/disconnected" => "thread_environment",
        "hook/started" | "hook/completed" => "hook",
        "model/rerouted" => "model_rerouted",
        "model/verification" => "model_verification",
        "model/safetyBuffering/updated" => "model_safety_buffering",
        "modelProvider/authRecoveryStarted" | "modelProvider/authRecoveryCompleted" => {
            "model_auth_recovery"
        }
        "skills/changed" => "skills_changed",
        "app/list/updated" => "apps_changed",
        "guardianWarning" => "guardian_warning",
        "deprecationNotice" => "deprecation_notice",
        "error" => "error",
        _ => "unknown",
    }
}
