//! Explicit v2 operator history capture. Original v1 plans/receipts are unchanged.
//! Source data is captured before fencing; later task access uses canonical data.
use super::*;
mod capture;
mod preview;
pub use preview::OperatorTaskTransferPreview;
mod receipt;
mod staging;
mod types;
pub(super) use capture::capture;
#[cfg(feature = "resource-adoption")]
pub(super) use receipt::authorize_active;
pub(super) use staging::{import, save, verify, verify_import};
use types::{Comment, MAX_BYTES, MAX_ROWS, ROW_BYTES, Task};
pub(super) use types::{History, Summary};
