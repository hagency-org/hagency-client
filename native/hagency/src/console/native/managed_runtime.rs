//! Retains the exact supervised qualification process until observed cleanup.
//! No prompt, model turn, workspace tool, successor or capacity-release method.
mod launch;
use super::{NativeError, failure};
use crate::console::lifecycle::{DrainFailure, RetainedTask, TaskCustody};
use hagency_runtime::owned::ManagedProcessState;
use hagency_store::owner_resources::*;
pub(super) use launch::PreparedLaunch;
use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{oneshot, watch};

pub(super) type LiveCheck =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<(), NativeError>> + Send>> + Send + Sync>;
struct Entry {
    digest: String,
    cancel: watch::Sender<bool>,
    task: RetainedTask,
}
#[derive(Default)]
struct State {
    closed: bool,
    entries: BTreeMap<String, Entry>,
    inspections: Vec<watch::Sender<bool>>,
}
#[derive(Clone, Default)]
pub(super) struct Manager {
    state: Arc<Mutex<State>>,
    custody: TaskCustody,
}
pub(super) struct Started {
    result: oneshot::Receiver<Result<ManagedRuntimeRecordedProcess, NativeError>>,
}
impl Started {
    pub async fn qualification(self) -> Result<ManagedRuntimeRecordedProcess, NativeError> {
        self.result
            .await
            .map_err(|_| failure(409, "managed_runtime_outcome_unknown"))?
    }
}
impl Manager {
    pub fn close(&self) {
        self.custody.close();
        match self.state.lock() {
            Ok(mut s) => {
                s.closed = true;
                for e in s.entries.values() {
                    e.cancel.send_replace(true);
                }
                for cancel in &s.inspections {
                    cancel.send_replace(true);
                }
            }
            Err(_) => self.custody.unknown(),
        }
    }
    pub async fn drain(&self) -> Result<(), DrainFailure> {
        self.close();
        self.custody.drain().await
    }
    /// Retained metadata preflight has no agent identity/custody. Its original
    /// supervisor must be observed stopped before the final launch is returned.
    pub async fn prepare(
        &self,
        prepared: PreparedLaunch,
        deadline: Instant,
        check: LiveCheck,
    ) -> Result<PreparedLaunch, NativeError> {
        if !prepared.discovery_required() {
            return Ok(prepared);
        }
        let activity = self
            .custody
            .enter()
            .map_err(|_| failure(409, "managed_runtime_closed"))?;
        let result = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| failure(409, "managed_runtime_outcome_unknown"))?;
            state.inspections.retain(|s| s.receiver_count() > 0);
            if state.closed || state.inspections.len() >= 128 {
                return Err(failure(409, "managed_runtime_closed"));
            }
            let (cancel, receiver) = watch::channel(false);
            let (sender, result) = oneshot::channel();
            let task =
                tokio::spawn(
                    async move { inspect(prepared, deadline, check, receiver, sender).await },
                );
            self.custody.track_result(task);
            state.inspections.push(cancel);
            result
        };
        drop(activity);
        result
            .await
            .map_err(|_| failure(409, "managed_runtime_outcome_unknown"))?
    }
    pub fn start(
        &self,
        registry: OwnerResourceRegistry,
        custody: ManagedRuntimeCustody,
        launch: PreparedLaunch,
        check: LiveCheck,
    ) -> Result<Started, NativeError> {
        let activity = match self.custody.enter() {
            Ok(a) => a,
            Err(_) => return self.refused(registry, custody, "managed_runtime_closed"),
        };
        let mut s = match self.state.lock() {
            Ok(s) => s,
            Err(_) => return self.refused(registry, custody, "managed_runtime_outcome_unknown"),
        };
        let id = custody.command().agent_reservation_id.clone();
        if s.closed || s.entries.contains_key(&id) || s.entries.len() >= 128 {
            drop(s);
            return self.refused(registry, custody, "managed_runtime_closed");
        }
        let digest = serde_json::to_value(custody.command())
            .ok()
            .and_then(|v| hagency_core::canonical::digest(&v).ok());
        let Some(digest) = digest else {
            drop(s);
            return self.refused(registry, custody, "managed_runtime_outcome_unknown");
        };
        let (cancel, receiver) = watch::channel(false);
        let (sender, result) = oneshot::channel();
        let task =
            tokio::spawn(
                async move { run(registry, custody, launch, check, receiver, sender).await },
            );
        let task = self.custody.track_result(task);
        s.entries.insert(
            id,
            Entry {
                digest,
                cancel,
                task,
            },
        );
        drop(s);
        drop(activity);
        Ok(Started { result })
    }
    fn refused(
        &self,
        registry: OwnerResourceRegistry,
        custody: ManagedRuntimeCustody,
        code: &'static str,
    ) -> Result<Started, NativeError> {
        self.custody.unknown();
        self.custody.cleanup(async move {
            let _ = registry.record_managed_runtime_unresolved(custody).await;
            Err(DrainFailure::Unknown)
        });
        Err(failure(409, code))
    }
    pub async fn stop(&self, id: &str, provision_digest: &str) -> Result<(), DrainFailure> {
        let task = {
            let s = self.state.lock().map_err(|_| DrainFailure::Unknown)?;
            let Some(e) = s.entries.get(id) else {
                return Ok(());
            };
            if e.digest != provision_digest {
                return Err(DrainFailure::Unknown);
            }
            e.cancel.send_replace(true);
            e.task.clone()
        };
        task.join().await
    }
}
async fn guarded<T>(
    cancel: &mut watch::Receiver<bool>,
    deadline: Instant,
    future: impl Future<Output = Result<T, NativeError>>,
) -> Result<T, NativeError> {
    if *cancel.borrow() || Instant::now() >= deadline {
        return Err(failure(409, "managed_runtime_closed"));
    }
    tokio::select! { biased;
        _ = cancel.changed() => Err(failure(409,"managed_runtime_closed")),
        _ = tokio::time::sleep_until(deadline.into()) => Err(failure(409,"managed_runtime_lease_expired")),
        result = future => result,
    }
}
fn store(e: hagency_store::Error) -> NativeError {
    super::management::store_error(e)
}
async fn inspect(
    mut prepared: PreparedLaunch,
    deadline: Instant,
    check: LiveCheck,
    mut cancel: watch::Receiver<bool>,
    sender: oneshot::Sender<Result<PreparedLaunch, NativeError>>,
) -> Result<(), DrainFailure> {
    if let Err(error) = guarded(&mut cancel, deadline, check()).await {
        let _ = sender.send(Err(error));
        return Ok(());
    }
    let mut process = match prepared.spawn(deadline) {
        Ok(p) => p,
        Err(error) => {
            let _ = sender.send(Err(error));
            return Err(DrainFailure::Unknown);
        }
    };
    let source = process.managed_process_source();
    let discovered = guarded(&mut cancel, deadline, async {
        check().await?;
        process
            .initialize()
            .await
            .map_err(|error| {
                tracing::debug!(target: "hagency_runtime::owned::managed", ?error, stage = "initialize", "managed inspection rejected");
                failure(409, "managed_runtime_policy_inspection_failed")
            })?;
        check().await?;
        tracing::debug!(target: "hagency_runtime::owned::managed", request = process.last_server_request(), "managed inspector initialized");
        let result = prepared
            .discover(&mut process)
            .await
            .map_err(|error| {
                tracing::debug!(target: "hagency_runtime::owned::managed", ?error, stage = "configuration", "managed inspection rejected");
                failure(409, "managed_runtime_policy_inspection_failed")
            })?;
        check().await?;
        Ok(result)
    })
    .await;
    let stopped = process.stop_managed_observation();
    if stopped.state() != ManagedProcessState::Stopped || !source.same(stopped.source()) {
        let _ = sender.send(Err(failure(
            409,
            "managed_runtime_inspection_cleanup_unknown",
        )));
        return Err(DrainFailure::Unknown);
    }
    let result = match discovered {
        Ok(observed) if source.same(observed.source()) => {
            match guarded(&mut cancel, deadline, check()).await {
                Ok(()) => prepared.apply_discovery(&observed).map(|()| prepared),
                Err(error) => Err(error),
            }
        }
        Ok(_) => Err(failure(409, "managed_runtime_policy_inspection_failed")),
        Err(error) => Err(error),
    };
    let _ = sender.send(result);
    Ok(())
}
async fn run(
    registry: OwnerResourceRegistry,
    custody: ManagedRuntimeCustody,
    launch: PreparedLaunch,
    check: LiveCheck,
    mut cancel: watch::Receiver<bool>,
    sender: oneshot::Sender<Result<ManagedRuntimeRecordedProcess, NativeError>>,
) -> Result<(), DrainFailure> {
    let deadline = custody.deadline();
    let initial = guarded(&mut cancel, deadline, async {
        registry
            .check_managed_runtime_custody(&custody)
            .await
            .map_err(store)?;
        check().await?;
        registry
            .check_managed_runtime_custody(&custody)
            .await
            .map_err(store)
    })
    .await;
    if let Err(error) = initial {
        let _ = registry.record_managed_runtime_unresolved(custody).await;
        let _ = sender.send(Err(error));
        return Err(DrainFailure::Unknown);
    }
    let mut process = match launch.spawn(deadline) {
        Ok(process) => process,
        Err(error) => {
            let _ = registry.record_managed_runtime_unresolved(custody).await;
            let _ = sender.send(Err(error));
            return Err(DrainFailure::Unknown);
        }
    };
    let sink =
        match registry.bind_managed_runtime_process(custody, process.managed_process_source()) {
            Ok(sink) => sink,
            Err(error) => {
                process.stop_managed_observation();
                let _ = sender.send(Err(store(error)));
                return Err(DrainFailure::Unknown);
            }
        };
    let qualification = guarded(&mut cancel, deadline, async {
        check().await?;
        registry
            .check_managed_runtime_process(&sink)
            .await
            .map_err(store)?;
        process
            .initialize()
            .await
            .map_err(|_| failure(409, "managed_runtime_qualification_failed"))?;
        check().await?;
        registry
            .check_managed_runtime_process(&sink)
            .await
            .map_err(store)?;
        let observation = process
            .qualify_managed_no_tools(launch.instructions())
            .await
            .map_err(|_| failure(409, "managed_runtime_qualification_failed"))?;
        check().await?;
        registry
            .record_managed_runtime_process(&sink, observation)
            .await
            .map_err(store)
    })
    .await;
    let qualified = qualification.is_ok();
    let _ = sender.send(qualification);
    if qualified {
        loop {
            let result = guarded(&mut cancel, deadline, async {
                tokio::time::sleep(Duration::from_millis(250)).await;
                check().await?;
                registry
                    .check_managed_runtime_process(&sink)
                    .await
                    .map_err(store)?;
                process
                    .qualify_ready_owner(Duration::from_millis(1))
                    .map_err(|_| failure(409, "managed_runtime_process_lost"))
            })
            .await;
            if result.is_err() {
                break;
            }
        }
    }
    // Deliberately outside expired live authority: retained evidence can record
    // only the exact original supervisor result, never authorize another spawn.
    let stopped = process.stop_managed_observation();
    let known = stopped.state() == ManagedProcessState::Stopped;
    registry
        .record_managed_runtime_process(&sink, stopped)
        .await
        .map_err(|_| DrainFailure::Unknown)?;
    if known {
        Ok(())
    } else {
        Err(DrainFailure::Unknown)
    }
}
