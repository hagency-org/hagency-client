use super::*;
use crate::console::lifecycle::DrainFailure;

impl OwnedRuntime {
    pub(in crate::console) fn close_for_migration(&self) {
        self.tasks.close();
        self.request_stop_all();
    }
    pub(in crate::console) async fn drain_for_migration(&self) -> Result<(), DrainFailure> {
        self.close_for_migration();
        // Ordinary shutdown may already have removed an entry. Its original
        // task is still in TaskCustody and concurrent/cancelled joins share the
        // same observation, including panic/abort failures.
        self.stop_all().await;
        self.tasks.drain().await
    }
}

#[cfg(test)]
mod tests;
