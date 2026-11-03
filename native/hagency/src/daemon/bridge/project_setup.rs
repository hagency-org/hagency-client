//! Typed facade for native project setup. Durable originals stay daemon-side.
use super::{ManagementClient, Operation, decode};
use crate::native_owner::{self as native, NativeError};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "camelCase", deny_unknown_fields)]
pub enum SetupOperation {
    Progress {
        project: String,
    },
    Begin {
        input: native::BeginSetup,
    },
    Continue {
        project: String,
        candidates: native::SetupCandidates,
    },
    Resume {
        project: String,
    },
    Cancel {
        project: String,
        setup: String,
        revision: u64,
    },
    Rebase {
        input: native::RebaseSetup,
    },
    Retry {
        command: String,
    },
    Acknowledge {
        command: String,
    },
    Assign {
        input: native::AssignAdministrator,
    },
}
impl SetupOperation {
    pub fn is_mutation(&self) -> bool {
        !matches!(self, Self::Progress { .. })
    }
}
pub struct ProjectSetupTransport {
    manager: Arc<ManagementClient>,
}
impl ProjectSetupTransport {
    pub fn new(manager: Arc<ManagementClient>) -> Self {
        Self { manager }
    }
    async fn call(&self, call: SetupOperation) -> Result<native::SetupProgress, NativeError> {
        match self.manager.as_ref() {
            ManagementClient::Local(manager) => dispatch_setup(manager.clone(), call).await,
            ManagementClient::Remote(manager) => {
                decode(manager.invoke(Operation::project_setup, call).await?)
            }
        }
    }
    pub async fn progress(&self, project: String) -> Result<native::SetupProgress, NativeError> {
        self.call(SetupOperation::Progress { project }).await
    }
    pub async fn begin_create(
        &self,
        input: native::BeginSetup,
    ) -> Result<native::SetupProgress, NativeError> {
        self.call(SetupOperation::Begin { input }).await
    }
    pub async fn continue_setup(
        &self,
        project: String,
        candidates: native::SetupCandidates,
    ) -> Result<native::SetupProgress, NativeError> {
        self.call(SetupOperation::Continue {
            project,
            candidates,
        })
        .await
    }
    pub async fn resume_begin(
        &self,
        project: String,
    ) -> Result<native::SetupProgress, NativeError> {
        self.call(SetupOperation::Resume { project }).await
    }
    pub async fn cancel(
        &self,
        project: String,
        setup: String,
        revision: u64,
    ) -> Result<native::SetupProgress, NativeError> {
        self.call(SetupOperation::Cancel {
            project,
            setup,
            revision,
        })
        .await
    }
    pub async fn rebase(
        &self,
        input: native::RebaseSetup,
    ) -> Result<native::SetupProgress, NativeError> {
        self.call(SetupOperation::Rebase { input }).await
    }
    pub async fn retry_command(
        &self,
        command: String,
    ) -> Result<native::SetupProgress, NativeError> {
        self.call(SetupOperation::Retry { command }).await
    }
    pub async fn acknowledge_failure(
        &self,
        command: String,
    ) -> Result<native::SetupProgress, NativeError> {
        self.call(SetupOperation::Acknowledge { command }).await
    }
    pub async fn assign_administrator(
        &self,
        input: native::AssignAdministrator,
    ) -> Result<native::SetupProgress, NativeError> {
        self.call(SetupOperation::Assign { input }).await
    }
}
pub(super) async fn dispatch_setup(
    manager: Arc<native::NativeManagement>,
    call: SetupOperation,
) -> Result<native::SetupProgress, NativeError> {
    let transport = native::ProjectSetupTransport::new(manager);
    match call {
        SetupOperation::Progress { project } => transport.progress(project).await,
        SetupOperation::Begin { input } => transport.begin_create(input).await,
        SetupOperation::Continue {
            project,
            candidates,
        } => transport.continue_setup(project, candidates).await,
        SetupOperation::Resume { project } => transport.resume_begin(project).await,
        SetupOperation::Cancel {
            project,
            setup,
            revision,
        } => transport.cancel(project, setup, revision).await,
        SetupOperation::Rebase { input } => transport.rebase(input).await,
        SetupOperation::Retry { command } => transport.retry_command(command).await,
        SetupOperation::Acknowledge { command } => transport.acknowledge_failure(command).await,
        SetupOperation::Assign { input } => transport.assign_administrator(input).await,
    }
}
