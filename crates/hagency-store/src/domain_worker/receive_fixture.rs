//! Explicit offline-fixture setup, retaining one original writer receipt.
//! The production bounded RPC path is deliberately not routed through this API.
use super::*;
use crate::{ReceiveWrite, domain::received_files as received};
use hagency_core::received_files::{ReceivedFileFacts, receive_limit};

impl DomainStore {
    /// Reserve and begin one original receive on the existing writer, for an
    /// explicit offline test fixture after its real Started handoff.
    ///
    /// Available only with `test-fixture`. This preserves the actual capability,
    /// current Started scope, facts, one-shot grant and bounded queue/byte budget.
    /// It waits for the original receipt rather than the production two-second
    /// RPC observation window. Callers must use a finite fixture watchdog; loss
    /// of observation never authorizes replay or grants success after timeout.
    pub async fn fixture_receive_write_once(
        &self,
        cap: RunnerCapability,
        event_id: String,
        limit: usize,
        facts: ReceivedFileFacts,
    ) -> Result<ReceiveWrite, Error> {
        crate::domain::file_delivery::cap_input(&cap)?;
        hagency_core::replies::matrix_event(&event_id)?;
        receive_limit(limit)?;
        facts.validate(limit)?;
        let bytes = weight(&(&cap, &event_id, limit, &facts))?;
        let permit = self
            .bytes
            .clone()
            .try_acquire_many_owned(bytes)
            .map_err(|_| Error::Busy)?;
        let ticket = self.progress.next.fetch_add(1, Ordering::AcqRel);
        let progress = self.progress.clone();
        let (reply, receipt) = oneshot::channel();
        self.tx
            .try_send(Job::Run {
                operation: Box::new(move |db| {
                    progress
                        .started
                        .store(ticket.wrapping_add(1), Ordering::Release);
                    if reply.is_closed() {
                        return;
                    }
                    let result = db.upload_transaction(writer_time, |tx, now| {
                        let original = received::reserve(tx, &cap, &event_id, limit, now)?;
                        let reservation = original.reservation.ok_or(Error::OutcomeUnknown)?;
                        received::begin(tx, &cap, &reservation, &facts, now)
                    });
                    let _ = reply.send(result);
                }),
                _bytes: permit,
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => Error::Busy,
                mpsc::error::TrySendError::Closed(_) => Error::Unavailable,
            })?;
        receipt.await.map_err(|_| Error::Unavailable)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hagency_core::received_files::ReceivedFileState;
    use rusqlite::trace::{TraceEvent, TraceEventCodes};
    use std::{cell::RefCell, future::Future, path::Path};

    struct Pause {
        entered: oneshot::Sender<()>,
        resume: std::sync::mpsc::Receiver<()>,
    }
    thread_local! {
        static PAUSE: RefCell<Option<Pause>> = const { RefCell::new(None) };
    }
    fn trace(event: TraceEvent<'_>) {
        let TraceEvent::Stmt(_, sql) = event else {
            return;
        };
        if !sql.starts_with("UPDATE received_files SET state='write_possible'") {
            return;
        }
        PAUSE.with(|slot| {
            if let Some(pause) = slot.borrow_mut().take() {
                let _ = pause.entered.send(());
                pause.resume.recv_timeout(Duration::from_secs(10)).unwrap();
            }
        });
    }
    async fn bounded<T>(future: impl Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(10), future)
            .await
            .expect("original fixture IO exceeded its watchdog")
    }
    // Private unit-test queue access, never a public arbitrary DB callback.
    async fn original<T: Send + 'static>(
        store: &DomainStore,
        operation: impl FnOnce(&mut DomainRepository) -> T + Send + 'static,
    ) -> T {
        let (reply, receipt) = oneshot::channel();
        store
            .tx
            .try_send(Job::Run {
                operation: Box::new(move |db| {
                    let _ = reply.send(operation(db));
                }),
                _bytes: store.bytes.clone().try_acquire_owned().unwrap(),
            })
            .unwrap_or_else(|_| panic!("original unit-test job was not admitted"));
        bounded(receipt).await.unwrap()
    }
    fn store(path: &Path) -> (DomainStore, RunnerCapability) {
        // Domain-level fixture data, not a claim of a physical child Started.
        // Execution's receive suite separately obtains its real StartedWorkspace.
        let (mut db, cap) = super::super::clock_tests::owned_fixture_with_attachment(path, true);
        let scope = db
            .owned_dispatch_scope(&cap, writer_time().unwrap())
            .unwrap();
        db.start_owned_dispatch(&cap, scope.fingerprint(), writer_time().unwrap())
            .unwrap();
        (DomainStore::start(db, 16).unwrap(), cap)
    }
    async fn pause_begin(
        store: &DomainStore,
    ) -> (oneshot::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (entered, reached) = oneshot::channel();
        let (resume, waiting) = std::sync::mpsc::channel();
        original(store, move |db| {
            db.upload_transaction(writer_time, |tx, _| {
                PAUSE.with(|slot| {
                    *slot.borrow_mut() = Some(Pause {
                        entered,
                        resume: waiting,
                    })
                });
                tx.trace_v2(TraceEventCodes::SQLITE_TRACE_STMT, Some(trace));
                Ok(())
            })
            .unwrap();
        })
        .await;
        (reached, resume)
    }
    fn facts() -> ReceivedFileFacts {
        ReceivedFileFacts {
            size: 3,
            sha256: hagency_core::project::hash(b"abc"),
        }
    }

    #[tokio::test]
    async fn native_receive_fixture_production_timeout_keeps_unknown() {
        let root = tempfile::tempdir().unwrap();
        let (store, cap) = store(root.path());
        let original_cap = cap.clone();
        let admission = original(&store, move |db| {
            db.reserve_received_file(&original_cap, "$receive", 1024, writer_time().unwrap())
                .unwrap()
        })
        .await;
        assert_eq!(admission.observation.state, ReceivedFileState::Reserved);
        let id = admission.identity.id().to_owned();
        let reservation = admission.reservation.unwrap();
        let (reached, resume) = pause_begin(&store).await;
        let request = {
            let store = store.clone();
            let cap = cap.clone();
            tokio::spawn(async move {
                store
                    .start_received_file_write(cap, reservation, facts())
                    .await
            })
        };
        bounded(reached).await.unwrap();
        assert!(matches!(
            bounded(request).await.unwrap(),
            Err(Error::OutcomeUnknown)
        ));
        assert_eq!(store.last_unknown_dequeued(), Some(true));
        // No returned grant, no retry and no assumption of rollback. The
        // original SQL operation is still paused before its actual UPDATE.
        resume.send(()).unwrap();
        let observed = original(&store, move |db| {
            db.inspect_received_file(&cap, &id).unwrap()
        })
        .await;
        assert_eq!(observed.state, ReceivedFileState::WritePossible);
        store.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn native_receive_fixture_retains_original_single_receipt() {
        let root = tempfile::tempdir().unwrap();
        let (store, cap) = store(root.path());
        let (reached, resume) = pause_begin(&store).await;
        let request = {
            let store = store.clone();
            let cap = cap.clone();
            tokio::spawn(async move {
                store
                    .fixture_receive_write_once(cap, "$receive".into(), 1024, facts())
                    .await
            })
        };
        bounded(reached).await.unwrap();
        // An actual production RPC behind the held original operation reaches
        // its unchanged two-second bound. It remains Unknown, never success.
        assert!(matches!(
            store.owned_dispatch_scope(cap.clone()).await,
            Err(Error::OutcomeUnknown)
        ));
        assert_eq!(store.last_unknown_dequeued(), Some(false));
        assert!(
            !request.is_finished(),
            "the fixture must retain its original reply observer"
        );
        resume.send(()).unwrap();
        let write = bounded(request).await.unwrap().unwrap();
        assert!(write.matches_capability(&cap));
        assert_eq!(write.facts(), &facts());
        let id = write.identity().id().to_owned();
        let inspected = original(&store, move |db| {
            let row = db.inspect_received_file(&cap, &id).unwrap();
            let count: u64 = db
                .upload_transaction(writer_time, |tx, _| {
                    Ok(tx.query_row("SELECT COUNT(*) FROM received_files", [], |r| r.get(0))?)
                })
                .unwrap();
            (row, count)
        })
        .await;
        assert_eq!(inspected.0.state, ReceivedFileState::WritePossible);
        assert_eq!(
            inspected.1, 1,
            "one real grant must not be duplicated or recovered by replay"
        );
        drop(write);
        store.shutdown().await.unwrap();
    }
}
