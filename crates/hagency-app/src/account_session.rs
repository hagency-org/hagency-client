//! Account-scoped backend tasks. Closing a scope aborts and drains every task
//! before UI cleanup and before opening the next account's scope.
use std::{
    future::Future,
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::{
    sync::oneshot,
    task::{AbortHandle, JoinHandle},
};

static EPOCH: AtomicU64 = AtomicU64::new(0);
static UPDATE_FROZEN: AtomicBool = AtomicBool::new(false);
static TASKS: LazyLock<Mutex<Tasks>> = LazyLock::new(|| {
    Mutex::new(Tasks {
        open: true,
        tasks: Vec::new(),
    })
});
struct Tasks {
    open: bool,
    tasks: Vec<(AbortHandle, oneshot::Receiver<()>)>,
}
struct Finished(Option<oneshot::Sender<()>>);
impl Drop for Finished {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}
pub fn epoch() -> u64 {
    EPOCH.load(Ordering::Acquire)
}

pub(crate) fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let mut scope = TASKS.lock().unwrap();
    scope.tasks.retain(|(handle, _)| !handle.is_finished());
    let (sender, receiver) = oneshot::channel();
    // Capture the guard outside the async body, so even an unpolled aborted task
    // notifies the drain barrier.
    let finished = Finished(Some(sender));
    let permitted = scope.open;
    let handle = crate::sliding_sync::backend_runtime().spawn(async move {
        let _finished = finished;
        if !permitted {
            std::future::pending::<()>().await;
        }
        future.await
    });
    if !scope.open {
        handle.abort();
    }
    scope.tasks.push((handle.abort_handle(), receiver));
    handle
}

pub(crate) async fn close() {
    {
        // Serialize the epoch boundary against an OAuth session commit/refresh.
        // Persistence never takes TASKS while holding this lock.
        // A user-selected Save As publication can already be in an OS rename
        // on a slow filesystem. Keep its original strong commit fence, but do
        // not block a Tokio worker while waiting for that original IO to return.
        let _session_io = loop {
            match crate::persistence::SESSION_IO.try_lock() {
                Ok(guard) => break guard,
                Err(std::sync::TryLockError::WouldBlock) => {}
                Err(std::sync::TryLockError::Poisoned(error)) => {
                    panic!("Session IO lock poisoned: {error}")
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        let mut scope = TASKS.lock().unwrap();
        scope.open = false;
        EPOCH.fetch_add(1, Ordering::AcqRel);
    }
    loop {
        let tasks = std::mem::take(&mut TASKS.lock().unwrap().tasks);
        if tasks.is_empty() {
            break;
        }
        for (handle, _) in &tasks {
            handle.abort();
        }
        for (_, finished) in tasks {
            let _ = finished.await;
        }
    }
    crate::settings::crypto_settings::clear_session();
}
pub(crate) fn open() {
    TASKS.lock().unwrap().open = !UPDATE_FROZEN.load(Ordering::Acquire);
}

/// Install preparation owns this process from now on. Drain account jobs after
/// the Agent and sync barriers, and prevent a foreground/login event reopening it.
#[cfg(all(target_os = "macos", feature = "macos-sparkle"))]
pub(crate) async fn close_for_update() {
    UPDATE_FROZEN.store(true, Ordering::Release);
    close().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    static TRANSITION_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    #[tokio::test]
    async fn transition_drains_old_work_and_rejects_launches_until_next_account() {
        let _serial = TRANSITION_TESTS.lock().await;
        open();
        let old_epoch = epoch();
        let (started, running) = oneshot::channel();
        let (late, mut result) = oneshot::channel();
        let old = spawn(async move {
            let _ = started.send(());
            std::future::pending::<()>().await;
            let _ = late.send("old account");
        });
        running.await.unwrap();
        close().await;
        assert!(old.await.unwrap_err().is_cancelled());
        assert!(result.try_recv().is_err());
        assert_ne!(epoch(), old_epoch);
        let rejected = spawn(async { "must not run" });
        close().await;
        assert!(rejected.await.unwrap_err().is_cancelled());
        open();
        assert_eq!(spawn(async { "new account" }).await.unwrap(), "new account");
    }
    #[tokio::test(flavor = "current_thread")]
    async fn closing_yields_current_thread_runtime_while_original_publication_holds_session_io() {
        let _serial = TRANSITION_TESTS.lock().await;
        open();
        let (entered, holding) = std::sync::mpsc::sync_channel(1);
        let (release, released) = std::sync::mpsc::sync_channel(1);
        let original_io = std::thread::spawn(move || {
            let _original = crate::persistence::SESSION_IO.lock().unwrap();
            entered.send(()).unwrap();
            // This is only a deadlock watchdog for a regressed blocking close.
            // Correctness depends on the explicit release after the first poll,
            // never on scheduling a heartbeat within a short wall-clock window.
            released
                .recv_timeout(std::time::Duration::from_secs(10))
                .is_ok()
        });
        holding.recv().unwrap();
        let mut closing = std::pin::pin!(close());
        let first_poll = closing
            .as_mut()
            .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()));
        let _ = release.send(());
        assert!(
            original_io.join().unwrap() && first_poll.is_pending(),
            "close blocked the single Tokio thread instead of yielding while original IO retained its commit fence"
        );
        tokio::time::timeout(std::time::Duration::from_secs(10), closing)
            .await
            .unwrap();
        open();
    }
}
