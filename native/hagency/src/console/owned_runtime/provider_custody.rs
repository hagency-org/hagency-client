//! A successful runtime-task join is insufficient when provider cleanup was lost.
use super::*;
use crate::console::lifecycle::{DrainFailure, TaskCustody};

pub(super) struct Provider {
    process: Option<runtime::Process>,
    tasks: TaskCustody,
}
impl Provider {
    pub async fn capture(
        tasks: &TaskCustody,
        future: impl std::future::Future<Output = codex::Result<runtime::Process>>,
    ) -> codex::Result<Self> {
        let creation = tasks.checked().map_err(|_| codex::Error::Unknown)?;
        // A cancelled/failed low-level constructor may already have launched
        // an inspection process. Until it supplies exit evidence, retain an
        // explicit unknown rather than interpreting its missing return as zero.
        let process = future.await?;
        creation.complete();
        Ok(Self {
            process: Some(process),
            tasks: tasks.clone(),
        })
    }
    pub async fn stop(&mut self) -> codex::Result<()> {
        let result = self
            .process
            .as_mut()
            .expect("provider custody")
            .stop()
            .await;
        if result.is_err() {
            self.tasks.unknown();
        }
        result
    }
}
impl std::ops::Deref for Provider {
    type Target = runtime::Process;
    fn deref(&self) -> &Self::Target {
        self.process.as_ref().expect("provider custody")
    }
}
impl std::ops::DerefMut for Provider {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.process.as_mut().expect("provider custody")
    }
}
impl Drop for Provider {
    fn drop(&mut self) {
        let Some(mut process) = self.process.take() else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_err() {
            self.tasks.unknown();
            return;
        }
        self.tasks
            .cleanup(async move { process.stop().await.map_err(|_| DrainFailure::Unknown) });
    }
}
