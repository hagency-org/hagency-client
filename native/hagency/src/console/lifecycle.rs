//! Retained join custody. Closing a producer is distinct from observing exit.
//! No receipt from this module proves provider-tree or ledger settlement.
use std::{
    future::Future,
    sync::{Arc, Mutex as StdMutex},
};
use tokio::{
    sync::{Mutex, Notify},
    task::{AbortHandle, JoinHandle},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DrainFailure {
    Closed,
    Unknown,
}

struct JoinState {
    task: Option<JoinHandle<Result<(), DrainFailure>>>,
    result: Option<Result<(), DrainFailure>>,
}

#[derive(Clone)]
pub(super) struct RetainedTask {
    state: Arc<Mutex<JoinState>>,
    abort: AbortHandle,
    id: tokio::task::Id,
}
impl RetainedTask {
    pub fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }
    pub fn is_finished(&self) -> bool {
        self.state.try_lock().is_ok_and(|s| {
            s.result.is_some() || s.task.as_ref().is_some_and(JoinHandle::is_finished)
        })
    }
    /// A finished handle is not cleanup evidence. Only the observed join result
    /// distinguishes successful cleanup from a panic, abort, or unresolved exit.
    pub fn observed_result(&self) -> Option<Result<(), DrainFailure>> {
        self.state.try_lock().ok().and_then(|state| state.result)
    }
    pub fn abort(&self) {
        self.abort.abort();
    }
    pub fn id(&self) -> tokio::task::Id {
        self.id
    }
    pub async fn join(&self) -> Result<(), DrainFailure> {
        let mut state = self.state.lock().await;
        if let Some(result) = state.result {
            return result;
        }
        // Await the retained handle by reference. Cancellation leaves both the
        // handle and its original result available to the next waiter.
        let result = match state.task.as_mut() {
            Some(task) => task.await.unwrap_or(Err(DrainFailure::Unknown)),
            None => Err(DrainFailure::Unknown),
        };
        state.result = Some(result);
        state.task.take();
        result
    }
}
impl std::future::IntoFuture for RetainedTask {
    type Output = Result<(), DrainFailure>;
    type IntoFuture = std::pin::Pin<Box<dyn Future<Output = Self::Output> + Send>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.join().await })
    }
}

#[derive(Default)]
struct State {
    closed: bool,
    active: usize,
    unknown: bool,
    tasks: Vec<RetainedTask>,
}
#[derive(Default)]
struct Inner {
    state: StdMutex<State>,
    changed: Notify,
}
#[derive(Clone, Default)]
pub(super) struct TaskCustody(Arc<Inner>);

pub(super) struct Activity {
    owner: TaskCustody,
}
pub(super) struct CheckedActivity {
    activity: Option<Activity>,
}
impl CheckedActivity {
    pub fn complete(mut self) {
        self.activity.take();
    }
}
impl Drop for CheckedActivity {
    fn drop(&mut self) {
        if let Some(activity) = &self.activity {
            activity.owner.unknown();
        }
    }
}
impl Drop for Activity {
    fn drop(&mut self) {
        if let Ok(mut state) = self.owner.0.state.lock() {
            state.active -= 1;
        }
        self.owner.0.changed.notify_waiters();
    }
}
impl TaskCustody {
    pub fn enter(&self) -> Result<Activity, DrainFailure> {
        let mut state = self.0.state.lock().map_err(|_| DrainFailure::Unknown)?;
        if state.closed {
            return Err(DrainFailure::Closed);
        }
        state.active = state.active.checked_add(1).ok_or(DrainFailure::Unknown)?;
        Ok(Activity {
            owner: self.clone(),
        })
    }
    pub fn close(&self) {
        if let Ok(mut state) = self.0.state.lock() {
            state.closed = true;
        }
        self.0.changed.notify_waiters();
    }
    pub fn check_open(&self) -> Result<(), DrainFailure> {
        let state = self.0.state.lock().map_err(|_| DrainFailure::Unknown)?;
        if state.closed {
            Err(DrainFailure::Closed)
        } else {
            Ok(())
        }
    }
    pub fn checked(&self) -> Result<CheckedActivity, DrainFailure> {
        Ok(CheckedActivity {
            activity: Some(self.enter()?),
        })
    }
    pub fn unknown(&self) {
        if let Ok(mut state) = self.0.state.lock() {
            state.unknown = true;
        }
        self.0.changed.notify_waiters();
    }
    /// Retain the exact cleanup result; observing task exit alone is insufficient.
    pub fn track_result(&self, task: JoinHandle<Result<(), DrainFailure>>) -> RetainedTask {
        let abort = task.abort_handle();
        let id = task.id();
        self.retain(task, abort, id)
    }
    pub fn track(&self, task: JoinHandle<()>) -> RetainedTask {
        let abort = task.abort_handle();
        let id = task.id();
        self.retain(
            tokio::spawn(async move { task.await.map_err(|_| DrainFailure::Unknown) }),
            abort,
            id,
        )
    }
    /// Background configuration polling is intentionally aborted on retirement.
    /// This observes task termination only; its provider children are tracked
    /// independently and cancelled constructors remain an unknown drain result.
    pub fn track_cancellable(&self, task: JoinHandle<()>) -> RetainedTask {
        let abort = task.abort_handle();
        let id = task.id();
        self.retain(
            tokio::spawn(async move {
                match task.await {
                    Ok(()) => Ok(()),
                    Err(error) if error.is_cancelled() => Ok(()),
                    Err(_) => Err(DrainFailure::Unknown),
                }
            }),
            abort,
            id,
        )
    }
    pub fn cleanup(&self, work: impl Future<Output = Result<(), DrainFailure>> + Send + 'static) {
        let task = tokio::spawn(work);
        let abort = task.abort_handle();
        let id = task.id();
        self.retain(task, abort, id);
    }
    fn retain(
        &self,
        task: JoinHandle<Result<(), DrainFailure>>,
        abort: AbortHandle,
        id: tokio::task::Id,
    ) -> RetainedTask {
        let task = RetainedTask {
            state: Arc::new(Mutex::new(JoinState {
                task: Some(task),
                result: None,
            })),
            abort,
            id,
        };
        match self.0.state.lock() {
            Ok(mut state) => {
                if !state.closed {
                    let mut failed = false;
                    state.tasks.retain(|old| {
                        match old.state.try_lock().ok().and_then(|s| s.result) {
                            Some(result) => {
                                failed |= result.is_err();
                                false
                            }
                            None => true,
                        }
                    });
                    state.unknown |= failed;
                }
                state.tasks.push(task.clone());
            }
            Err(_) => self.unknown(),
        }
        let observation = task.clone();
        tokio::spawn(async move {
            let _ = observation.join().await;
        });
        self.0.changed.notify_waiters();
        task
    }
    pub async fn drain(&self) -> Result<(), DrainFailure> {
        self.close();
        loop {
            let changed = self.0.changed.notified();
            let active = self
                .0
                .state
                .lock()
                .map_err(|_| DrainFailure::Unknown)?
                .active;
            if active == 0 {
                break;
            }
            changed.await;
        }
        self.settle().await
    }
    pub async fn settle(&self) -> Result<(), DrainFailure> {
        loop {
            let tasks = self
                .0
                .state
                .lock()
                .map_err(|_| DrainFailure::Unknown)?
                .tasks
                .clone();
            for task in &tasks {
                if task.join().await.is_err() {
                    self.unknown();
                }
            }
            let state = self.0.state.lock().map_err(|_| DrainFailure::Unknown)?;
            if state.tasks.iter().all(|task| {
                task.state
                    .try_lock()
                    .is_ok_and(|task| task.result.is_some())
            }) {
                return if state.unknown {
                    Err(DrainFailure::Unknown)
                } else {
                    Ok(())
                };
            }
        }
    }
}

#[cfg(test)]
mod tests;
