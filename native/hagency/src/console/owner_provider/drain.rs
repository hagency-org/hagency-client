//! Provider children retain reap custody even if their caller is cancelled.
use super::*;
use crate::console::lifecycle::{DrainFailure, TaskCustody};

pub(super) struct TrackedChild {
    child: Option<Child>,
    tasks: TaskCustody,
}
impl TrackedChild {
    pub fn new(child: Child, tasks: &TaskCustody) -> Self {
        Self {
            child: Some(child),
            tasks: tasks.clone(),
        }
    }
}
impl std::ops::Deref for TrackedChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        self.child.as_ref().expect("retained child")
    }
}
impl std::ops::DerefMut for TrackedChild {
    fn deref_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("retained child")
    }
}
impl Drop for TrackedChild {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_err() {
            self.tasks.unknown();
            return;
        }
        self.tasks.cleanup(async move {
            match child.try_wait() {
                Ok(Some(_)) => Ok(()),
                Ok(None) => child.kill().await.map_err(|_| DrainFailure::Unknown),
                Err(_) => Err(DrainFailure::Unknown),
            }
        });
    }
}
impl OwnerProvider {
    pub(in crate::console) fn close_for_migration(&self) {
        self.tasks.close();
        self.request_stop_all();
    }
    pub(in crate::console) async fn drain_for_migration(&self) -> Result<(), DrainFailure> {
        self.close_for_migration();
        self.stop_all().await;
        self.tasks.drain().await
    }
}

#[cfg(test)]
mod tests;
