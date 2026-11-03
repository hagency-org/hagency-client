//! One admission barrier for personal and pooled model turns.
use std::sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}};
use std::time::Duration;

#[derive(Default)]
pub struct Admission {
    open: AtomicBool,
    barrier: Arc<tokio::sync::RwLock<()>>,
    active: AtomicU64,
}

impl Admission {
    pub(crate) fn running() -> Self {
        let gate = Self::default();
        gate.open();
        gate
    }
    pub fn is_open(&self) -> bool { self.open.load(Ordering::Acquire) }
    pub fn open(&self) { self.open.store(true, Ordering::Release); }
    pub async fn close(&self, within: Duration) -> bool {
        self.open.store(false, Ordering::Release);
        tokio::time::timeout(within, self.barrier.write()).await.is_ok()
    }
    pub fn enter(&self) -> Option<tokio::sync::RwLockReadGuard<'_, ()>> {
        if !self.is_open() { return None; }
        let guard = self.barrier.try_read().ok()?;
        self.is_open().then_some(guard)
    }
    pub(crate) fn enter_turn(self: &Arc<Self>) -> Option<TurnAdmission> {
        if !self.is_open() { return None; }
        let barrier = self.barrier.clone().try_read_owned().ok()?;
        self.is_open().then(|| TurnAdmission { gate: self.clone(), barrier: Some(barrier), active: false })
    }
    pub fn active_turns(&self) -> u64 { self.active.load(Ordering::Acquire) }
    pub(crate) fn is_quiescent(&self) -> bool {
        !self.is_open() && self.barrier.try_write().is_ok()
    }
}

/// Holds the barrier through provider start acknowledgement. Once started,
/// pause may complete while the turn drains; dropping the lease updates the
/// count on success, error, cancellation, or unwind.
pub(crate) struct TurnAdmission {
    gate: Arc<Admission>,
    barrier: Option<tokio::sync::OwnedRwLockReadGuard<()>>,
    active: bool,
}
impl TurnAdmission {
    pub(crate) fn started(&mut self) {
        if !self.active {
            self.gate.active.fetch_add(1, Ordering::AcqRel);
            self.active = true;
        }
        self.barrier.take();
    }
}
impl Drop for TurnAdmission {
    fn drop(&mut self) {
        if self.active { self.gate.active.fetch_sub(1, Ordering::AcqRel); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn pause_waits_for_start_but_running_turns_drain_and_drop_exactly_once() {
        let gate = Arc::new(Admission::running());
        let mut turn = gate.enter_turn().unwrap();
        assert_eq!(gate.active_turns(), 0);
        assert!(!gate.close(Duration::from_millis(5)).await);
        assert!(!gate.is_quiescent());
        assert!(gate.enter_turn().is_none());
        turn.started();
        turn.started();
        assert!(gate.close(Duration::from_millis(5)).await);
        assert!(gate.is_quiescent());
        assert_eq!(gate.active_turns(), 1);
        drop(turn);
        assert_eq!(gate.active_turns(), 0);
        gate.open();
        let canceled = gate.enter_turn().unwrap();
        drop(canceled);
        assert_eq!(gate.active_turns(), 0);
        assert!(gate.close(Duration::from_millis(5)).await);
    }
}
