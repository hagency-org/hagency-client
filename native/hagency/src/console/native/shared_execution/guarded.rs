//! A retained phase boundary for the original, successor and budget drivers.
//! Constructing and polling large SDK/approval futures inline in each driver
//! stacks their debug frames until an ordinary Tokio worker can overflow.
use super::{NativeError, failure};
use std::{future::Future, pin::Pin, time::Instant};
use tokio::sync::watch;

type Phase<'a, T> = Pin<Box<dyn Future<Output = Result<T, NativeError>> + Send + 'a>>;

/// Defer construction until the cancellation/deadline gate has been checked.
/// This is the same awaited phase, never a detached task: dropping its caller
/// drops the active child and leaves the retained process custody to drain.
#[inline(never)]
pub(super) fn guarded<'a, T: 'a, M, F>(
    cancel: &'a mut watch::Receiver<bool>,
    until: Instant,
    make: M,
) -> Phase<'a, T>
where
    M: FnOnce() -> F + Send + 'a,
    F: Future<Output = Result<T, NativeError>> + Send + 'a,
{
    Box::pin(async move {
        if *cancel.borrow() || Instant::now() >= until {
            return Err(failure(409, "shared_execution_closed"));
        }
        // Creation is inside this heap-owned phase rather than the parent
        // driver's state machine; cancellation still wins the biased select.
        let future = make();
        tokio::select! {biased;
            _ = cancel.changed() => Err(failure(409, "shared_execution_closed")),
            _ = tokio::time::sleep_until(until.into()) => Err(failure(409, "shared_execution_expired")),
            result = future => result,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
    use std::time::Duration;

    #[tokio::test]
    async fn shared_phase_does_not_construct_after_retirement_or_expiry() {
        for retired in [false, true] {
            let (_send, mut cancel) = watch::channel(retired);
            let constructed = AtomicBool::new(false);
            let until = if retired { Instant::now()+Duration::from_secs(1) } else { Instant::now() };
            let result = guarded(&mut cancel, until, || {
                constructed.store(true, Ordering::SeqCst);
                async { Ok(()) }
            }).await;
            assert_eq!(result.unwrap_err().code, "shared_execution_closed");
            assert!(!constructed.load(Ordering::SeqCst));
        }
    }

    #[tokio::test]
    async fn shared_phase_cancellation_drops_original_child_before_returning() {
        struct Dropped(Arc<AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) { self.0.store(true, Ordering::SeqCst); }
        }
        let (send, mut cancel) = watch::channel(false);
        let dropped = Arc::new(AtomicBool::new(false));
        let observed = dropped.clone();
        let entered = Arc::new(tokio::sync::Notify::new());
        let wake = entered.clone();
        let task = tokio::spawn(async move {
            guarded(&mut cancel, Instant::now()+Duration::from_secs(5), || async move {
                let _original = Dropped(observed);
                wake.notify_one();
                std::future::pending::<()>().await;
                Ok(())
            }).await
        });
        entered.notified().await;
        send.send(true).unwrap();
        assert_eq!(task.await.unwrap().unwrap_err().code, "shared_execution_closed");
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shared_phase_large_child_does_not_expand_driver_future() {
        async fn large_child() -> Result<u8, NativeError> {
            let retained = [7u8; 256 * 1024];
            tokio::task::yield_now().await;
            Ok(std::hint::black_box(&retained)[0])
        }
        assert!(std::mem::size_of_val(&large_child()) >= 256 * 1024);
        tokio::spawn(async {
            let (_send, mut cancel) = watch::channel(false);
            let phase = guarded(&mut cancel, Instant::now()+Duration::from_secs(5), large_child);
            assert!(std::mem::size_of_val(&phase) <= 2 * std::mem::size_of::<usize>());
            assert_eq!(phase.await.unwrap(), 7);
        }).await.unwrap();
    }
}
