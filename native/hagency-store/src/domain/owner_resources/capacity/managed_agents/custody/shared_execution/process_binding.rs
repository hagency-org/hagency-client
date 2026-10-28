//! Closed original/successor process binding. This is never serialized and can
//! only contain a sink created from consumed one-shot native process custody.
use super::*;
#[derive(Clone)]
pub(super) enum ProcessBinding {
    Original(Arc<SharedProcessSink>),
    Attempt(Arc<SharedAttemptProcessSink>),
    #[cfg(feature = "shared-budget-execution")]
    Budget(Arc<SharedBudgetAttemptProcessSink>),
}
impl ProcessBinding {
    pub(super) fn is_successor(&self) -> bool {
        !matches!(self, Self::Original(_))
    }
    pub(super) fn source(&self) -> &hagency_runtime::owned::SharedTextProcessSource {
        match self {
            Self::Original(s) => &s.source,
            Self::Attempt(s) => &s.source,
            #[cfg(feature = "shared-budget-execution")]
            Self::Budget(s) => &s.source,
        }
    }
    pub(super) fn enrollment(&self) -> &VerifiedManagementEnrollment {
        match self {
            Self::Original(s) => &s.custody.enrollment,
            Self::Attempt(s) => &s.custody.enrollment,
            #[cfg(feature = "shared-budget-execution")]
            Self::Budget(s) => &s.custody.enrollment,
        }
    }
    pub(super) fn original(&self) -> &SharedRuntimeCommand {
        match self {
            Self::Original(s) => &s.custody.command,
            Self::Attempt(s) => &s.custody.command.original_runtime_command,
            #[cfg(feature = "shared-budget-execution")]
            Self::Budget(s) => &s.custody.command.original_runtime_command,
        }
    }
    pub(super) fn command_id(&self) -> &str {
        match self {
            Self::Original(s) => &s.custody.command.command_id,
            Self::Attempt(s) => &s.custody.command.command_id,
            #[cfg(feature = "shared-budget-execution")]
            Self::Budget(s) => &s.custody.command.command_id,
        }
    }
    pub(super) fn command_digest(&self) -> &str {
        match self {
            Self::Original(s) => &s.custody.digest,
            Self::Attempt(s) => &s.custody.digest,
            #[cfg(feature = "shared-budget-execution")]
            Self::Budget(s) => &s.custody.digest,
        }
    }
    pub(super) fn child(&self) -> &str {
        &self.original().agent_reservation_id
    }
    pub(super) fn registry(&self) -> &Arc<Inner> {
        match self {
            Self::Original(s) => &s.custody.scope.registry,
            Self::Attempt(s) => &s.custody.registry,
            #[cfg(feature = "shared-budget-execution")]
            Self::Budget(s) => &s.custody.registry,
        }
    }
    pub(super) fn provider(&self) -> &str {
        match self {
            Self::Original(s) => &s.custody.provider,
            Self::Attempt(s) => &s.custody.provider,
            #[cfg(feature = "shared-budget-execution")]
            Self::Budget(s) => &s.custody.provider,
        }
    }
    pub(super) fn process_digest(&self) -> Result<String, Error> {
        match self {
            Self::Original(s) => s.process_digest(),
            Self::Attempt(s) => s.process_digest(),
            #[cfg(feature = "shared-budget-execution")]
            Self::Budget(s) => s.process_digest(),
        }
    }
}
