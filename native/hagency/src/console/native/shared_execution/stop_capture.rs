//! Try to collect the original provider's final usage before stopping its tree.
//! An interrupt acknowledgement alone is never usage or permission to refund.
use super::*;
use hagency_runtime::{codex::session::{ObservationKind, Phase}, owned::OwnedSession};

pub(super) fn requested(cancel: &tokio::sync::watch::Receiver<bool>) -> bool {
    *cancel.borrow() || cancel.has_changed().is_err()
}

pub(super) async fn settle(
    process: &mut OwnedSession,
    capture: &mut SharedCallSink,
    usage: &mut crate::console::native::telemetry::TurnTelemetry,
    until: Instant,
) -> Option<SharedWorkReceipt> {
    if process.phase() != Phase::Running || until <= Instant::now() {
        return None;
    }
    // Cleanup fits inside the original authority, and the host's stop bound.
    let deadline = until.min(Instant::now() + Duration::from_secs(3));
    tokio::time::timeout_at(deadline.into(), async {
        process.interrupt().await.ok()?;
        loop {
            let event = process.next_shared_text_event().await.ok()?;
            let ended = matches!(event.observation().kind(), ObservationKind::TurnEnded(_));
            let pending = usage.pending(&event);
            let receipt = capture.observe(event).await.ok()?;
            usage.accept(pending);
            usage.quota(process);
            if ended { return receipt; }
        }
    }).await.ok().flatten()
}
