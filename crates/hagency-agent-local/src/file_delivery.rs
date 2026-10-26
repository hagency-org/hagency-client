//! Safe original-file delivery protocol. These values carry no byte custody,
//! local source path, media locator, or permission to upload/send/retry.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
pub const CAPABILITY: &str = "agent-file-delivery-v1";
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
pub const CHUNK_BYTES: usize = 64 * 1024;
fn key(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(Error::Invalid("file delivery identifier"));
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub filename: String,
    pub caption: Option<String>,
    pub bytes: u64,
    pub sha256: String,
}
impl Manifest {
    pub fn validate(&self) -> Result<()> {
        if self.filename.is_empty()
            || self.filename.len() > 255
            || self.filename.chars().any(char::is_control)
            || self.filename.contains(['/', '\\'])
            || matches!(self.filename.as_str(), "." | "..")
            || self
                .caption
                .as_ref()
                .is_some_and(|s| s.len() > 1000 || s.contains('\0'))
            || self.bytes > MAX_BYTES as u64
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Invalid("file delivery manifest"));
        }
        Ok(())
    }
    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        let bytes = serde_json::to_vec(&(&self.filename, &self.caption, self.bytes, &self.sha256))?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    pub dispatch_id: String,
    pub execution_id: String,
    pub file_id: String,
}
impl Target {
    pub fn validate(&self) -> Result<()> {
        for id in [&self.dispatch_id, &self.execution_id, &self.file_id] {
            key(id)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Delivery {
    pub id: String,
    pub dispatch_id: String,
    pub execution_id: String,
    pub request_id: String,
    pub created_at_ms: i64,
    pub manifest: Manifest,
    pub manifest_digest: String,
    pub received_bytes: u64,
    pub state: String,
    pub upload_accepted: bool,
    pub matrix_event_id: Option<String>,
}
impl Delivery {
    /// Checks historical consistency only. Even a valid uploaded/unknown status
    /// cannot mint a replacement attempt or any new upload/send authority.
    pub fn validate(
        &self,
        dispatch: &str,
        execution: &str,
        request: &str,
        manifest: &Manifest,
    ) -> Result<()> {
        for id in [
            &self.id,
            &self.dispatch_id,
            &self.execution_id,
            &self.request_id,
        ] {
            key(id)?;
        }
        let complete = matches!(
            self.state.as_str(),
            "queued" | "uploading" | "uploaded" | "sending" | "sent" | "unknown"
        );
        if self.dispatch_id != dispatch
            || self.execution_id != execution
            || self.request_id != request
            || &self.manifest != manifest
            || self.manifest_digest != manifest.digest()?
            || self.created_at_ms < 0
            || self.received_bytes > manifest.bytes
            || complete && self.received_bytes != manifest.bytes
            || !matches!(
                self.state.as_str(),
                "staging"
                    | "queued"
                    | "uploading"
                    | "uploaded"
                    | "sending"
                    | "sent"
                    | "unknown"
                    | "cancelled"
            )
            || (self.state == "sent") != self.matrix_event_id.is_some()
            || matches!(self.state.as_str(), "uploaded" | "sending" | "sent")
                && !self.upload_accepted
            || matches!(self.state.as_str(), "staging" | "queued" | "uploading")
                && self.upload_accepted
            || self.matrix_event_id.as_ref().is_some_and(|event| {
                !event.starts_with('$')
                    || event.len() > 512
                    || event.len() < 2
                    || event.chars().any(|c| c.is_control() || c.is_whitespace())
            })
        {
            return Err(Error::Invalid("file delivery response"));
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn original() -> Manifest {
        Manifest {
            filename: "original.txt".into(),
            caption: Some("Original caption".into()),
            bytes: 5,
            sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824".into(),
        }
    }
    #[test]
    fn commitments_match_server_tuple_and_bind_every_field() {
        let f = original();
        assert_eq!(
            f.digest().unwrap(),
            "428179ba2a063bcf2f87f98c7e25ec0c7345d26d76ae376c0232caef3a0f1f04"
        );
        for field in ["filename", "caption", "bytes", "sha256"] {
            let mut value = serde_json::to_value(&f).unwrap();
            value[field] = match field {
                "bytes" => 0.into(),
                "sha256" => "0".repeat(64).into(),
                _ => "changed".into(),
            };
            let changed: Manifest = serde_json::from_value(value).unwrap();
            assert_ne!(f.digest().unwrap(), changed.digest().unwrap());
        }
    }
    #[test]
    fn historical_status_rejects_changed_scope_bytes_and_private_fields() {
        let f = original();
        let base = serde_json::json!({"id":"fil_original","dispatchId":"dispatch","executionId":"execution","requestId":"request","createdAtMs":1,"manifest":f,"manifestDigest":f.digest().unwrap(),"receivedBytes":5,"state":"unknown","uploadAccepted":true,"matrixEventId":null});
        let valid: Delivery = serde_json::from_value(base.clone()).unwrap();
        valid
            .validate("dispatch", "execution", "request", &f)
            .unwrap();
        for (field, value) in [
            ("id", serde_json::json!("../other")),
            ("dispatchId", serde_json::json!("other")),
            ("executionId", serde_json::json!("other")),
            ("requestId", serde_json::json!("other")),
            ("manifestDigest", serde_json::json!("0".repeat(64))),
            ("receivedBytes", serde_json::json!(4)),
            ("state", serde_json::json!("sent")),
            ("matrixEventId", serde_json::json!("$fabricated:test")),
        ] {
            let mut changed = base.clone();
            changed[field] = value;
            assert!(
                serde_json::from_value::<Delivery>(changed)
                    .unwrap()
                    .validate("dispatch", "execution", "request", &f)
                    .is_err()
            );
        }
        let mut private = base;
        private["mxcUrl"] = "mxc://private.test/hidden".into();
        assert!(serde_json::from_value::<Delivery>(private).is_err());
    }
}
