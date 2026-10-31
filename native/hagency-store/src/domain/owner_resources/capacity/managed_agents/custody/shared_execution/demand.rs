//! Isolated wire-pending durable event demand. Descriptive offer/start/promotion
//! custody is distinct from fresh process/activity/model authority.
use super::*;
mod guard;
pub(super) mod journal;
mod promotion;
mod recovery;
mod stop;
mod submission;
mod types;
pub use stop::{SharedDemandStopInput, SharedDemandStopIntent};
pub use submission::{SharedDemandRecovery, SharedDemandSubmission};
pub use types::*;

pub(super) use guard::{
    attempt_control_revision, check_attempt_control, require_explicit_start, require_no_owner_stop,
};
pub(super) use promotion::check_work_control;
