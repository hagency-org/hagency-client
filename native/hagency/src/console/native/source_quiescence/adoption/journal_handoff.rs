//! One handoff protocol for resource migration and empty-device initialization.
use super::*;
use hagency_store::management::ArchiveJournalBinding;

impl JournalCustody {
    pub(in crate::console::native::source_quiescence) async fn open_journal(
        self: &Arc<Self>,
        source: &Arc<QuiescedSources>,
        binding: ArchiveJournalBinding,
    ) -> Result<PrivateManagementJournal, NativeError> {
        let source = source.clone();
        let custody = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut saved = custody
                .journal
                .lock()
                .map_err(|_| err("source_adoption_custody_missing"))?;
            if saved.is_none() {
                custody.original_lock.store(false, Ordering::Release);
                let files = source
                    .files
                    .lock()
                    .map_err(|_| err("source_quiescence_unknown"))?;
                match files.journal(binding) {
                    Ok(journal) => *saved = Some(journal),
                    Err(error) => {
                        if files.reacquire().is_ok() {
                            custody.original_lock.store(true, Ordering::Release);
                        }
                        return Err(err(error));
                    }
                }
            }
            saved
                .as_ref()
                .filter(|j| j.writer_open())
                .cloned()
                .ok_or(err("source_adoption_custody_missing"))
        })
        .await
        .map_err(|_| err("source_adoption_outcome_unknown"))?
    }
    /// The retained JoinHandle lets a cancelled caller resume the same drain.
    pub(in crate::console::native::source_quiescence) async fn drain_journal(
        self: &Arc<Self>,
        source: &Arc<QuiescedSources>,
        state: &Mutex<JournalDrain>,
    ) -> Result<(), NativeError> {
        let mut drain = state.lock().await;
        if matches!(*drain, JournalDrain::Idle) {
            let journal = self
                .journal
                .lock()
                .map_err(|_| err("source_adoption_custody_missing"))?
                .clone();
            *drain = if let Some(journal) = journal {
                let source = source.clone();
                let custody = self.clone();
                JournalDrain::Running(tokio::spawn(async move {
                    journal.shutdown().await.map_err(store)?;
                    source
                        .files
                        .lock()
                        .map_err(|_| err("source_quiescence_unknown"))?
                        .reacquire()
                        .map_err(err)?;
                    custody
                        .journal
                        .lock()
                        .map_err(|_| err("source_adoption_custody_missing"))?
                        .take();
                    custody.original_lock.store(true, Ordering::Release);
                    Ok(())
                }))
            } else {
                JournalDrain::Finished(Ok(()))
            };
        }
        if let JournalDrain::Running(task) = &mut *drain {
            let result = task
                .await
                .unwrap_or_else(|_| Err(err("source_adoption_custody_unknown")))
                .map_err(|e| (e.status, e.code));
            *drain = JournalDrain::Finished(result);
        }
        let JournalDrain::Finished(result) = &*drain else {
            unreachable!()
        };
        result
            .clone()
            .map_err(|(status, code)| NativeError { status, code })
    }
}
