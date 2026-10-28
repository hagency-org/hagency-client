//! Explicit v3 resource-alert history transfer; existing v1/v2 bytes stay unchanged.
use super::*;
mod capture;
mod staging;
mod types;
pub(super) use capture::capture;
pub(super) use staging::{conflicts, import, verify, verify_import};
pub(super) use types::{History, Summary};
use types::{Alert, Note, MAX_ALERTS, MAX_NOTES, MAX_BYTES, ROW_BYTES};
