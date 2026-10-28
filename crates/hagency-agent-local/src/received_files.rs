//! Durable original-request file reception. A reservation mints custody once;
//! lost/cancelled writes cannot be retried by recreating a publisher. Metadata
//! contains no MXC/key. A ready path requires current file identity and readback.

use serde::{Deserialize, Serialize};

pub const CAPABILITY_VERSION: &str = "hagency-agent-receive-v1";
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
#[cfg(unix)]
const MAX_RECORDS: u64 = 4096;
#[cfg(unix)]
const MAX_HELD: u64 = 128 * 1024 * 1024;
pub(crate) const SCHEMA: &str = include_str!("received_files/schema.sql");
pub(crate) const OBJECTS: &[&str] = &[
    "received_files",
    "retain_received_file",
    "immutable_received_file",
];
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub relative_path: String,
    pub bytes: u64,
    pub sha256: String,
    // Evidence of the original physical object, not permission to open a path.
    identity: (u64, u64, i64, i64, i64, i64),
    workspace_identity: hagency_platform::DirectoryIdentity,
    parent_identity: hagency_platform::DirectoryIdentity,
}

#[cfg(unix)]
mod publisher;
#[cfg(unix)]
pub use publisher::{Begin, Custody, Prepared, Receiver};
#[cfg(all(test, unix))]
mod tests;
