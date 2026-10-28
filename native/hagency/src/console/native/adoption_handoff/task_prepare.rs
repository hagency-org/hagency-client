//! Original native v2 preparation is descriptive, not a source/registry grant.
//! It shares the initializer's atomic create-once file so no two-file crash gap
//! can lose the original device or silently select a replacement migration.
use super::*;
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::console::native) struct TaskAdoptionPrepare {
    pub source_binding: String,
    pub plan_digest: String,
    pub inventory_digest: String,
    pub monthly_account_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<TaskPrepareReview>,
}
/// Descriptive bounded preview only; effect authority remains the exact saved
/// digests plus canonical staging or original unfenced source recapture.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::console::native) struct TaskPrepareReview {
    pub resources: Vec<(String, String)>,
    pub tasks: u32,
    pub comments: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alerts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alert_notes: Option<u32>,
}
impl TaskPrepareReview {
    pub(in crate::console::native) fn from_summary(
        s: &hagency_store::owner_resources::adoption::AdoptionSummary,
    ) -> Result<Self, NativeError> {
        let value = Self {
            resources: s
                .resource_ids
                .iter()
                .cloned()
                .zip(s.resource_revisions.iter().cloned())
                .collect(),
            tasks: s.transferred_task_count.ok_or_else(descriptor_error)?,
            comments: s.transferred_comment_count.ok_or_else(descriptor_error)?,
            alerts: s.transferred_alert_count,
            alert_notes: s.transferred_alert_note_count,
        };
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<(), NativeError> {
        if self.resources.is_empty()
            || self.resources.len() > 16
            || self.tasks > 10_000
            || self.comments > 10_000
            || self.alerts.is_some() != self.alert_notes.is_some()
            || self.alerts.is_some_and(|n| n > 1_000)
            || self.alert_notes.is_some_and(|n| n > 10_000)
            || self.resources.iter().any(|(id, revision)| {
                key(id).is_err()
                    || revision.len() != 64
                    || !revision.bytes().all(|c| c.is_ascii_hexdigit())
            })
        {
            return Err(descriptor_error());
        }
        Ok(())
    }
}
impl TaskAdoptionPrepare {
    pub(super) fn validate(&self) -> Result<(), NativeError> {
        if [
            &self.source_binding,
            &self.plan_digest,
            &self.inventory_digest,
        ]
        .into_iter()
        .any(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
            || self.monthly_account_tokens == 0
            || self.monthly_account_tokens > 9_007_199_254_740_991
        {
            return Err(descriptor_error());
        }
        if let Some(review) = &self.review {
            review.validate()?;
        }
        Ok(())
    }
}
impl AdoptedManagementSelection {
    pub(in crate::console::native) fn task_preparation(&self) -> Option<&TaskAdoptionPrepare> {
        self.task_prepare.as_ref()
    }

    pub(in crate::console::native) fn prepared_tasks(
        owner: &NativeOwner,
        registry: OwnerResourceRegistry,
        migration_id: String,
        profile: String,
        device: String,
        prepare: TaskAdoptionPrepare,
    ) -> Result<Self, NativeError> {
        prepare.validate()?;
        let mut selected = Self::completed(owner, registry, migration_id, profile, device);
        selected.task_prepare = Some(prepare);
        Ok(selected)
    }
    pub(in crate::console::native) fn original_task_prepare(
        &self,
        device: &str,
    ) -> Result<&TaskAdoptionPrepare, NativeError> {
        if self.device != device {
            return Err(descriptor_error());
        }
        self.task_prepare.as_ref().ok_or_else(descriptor_error)
    }
}
impl AdoptedManagementSelection {
    pub(in crate::console::native) fn require_no_task_prepare(
        owner: &NativeOwner,
    ) -> Result<(), NativeError> {
        if read_descriptor(owner.0.host.state_directory())?
            .is_some_and(|d| d.task_prepare.is_some() || d.empty_prepare.is_some())
        {
            return Err(failure(409, "source_adoption_recovery_only"));
        }
        Ok(())
    }
}

/// A synced temporary becomes the initializer through the existing atomic
/// NOREPLACE primitive. An interrupted write is never the published descriptor,
/// and even another pending initializer cannot be overwritten.
pub(super) fn persist_new(state: &Path, bytes: &[u8]) -> Result<(), NativeError> {
    persist_new_with(state, bytes, || Ok(()))
}
fn persist_new_with(
    state: &Path,
    bytes: &[u8],
    before_publish: impl FnOnce() -> Result<(), NativeError>,
) -> Result<(), NativeError> {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
    use std::io::Write;
    let dir = cap_std::fs::Dir::open_ambient_dir(state, cap_std::ambient_authority())
        .map_err(|_| descriptor_error())?;
    hagency_store::private::check_handle(
        &dir.try_clone()
            .map_err(|_| descriptor_error())?
            .into_std_file(),
    )
    .map_err(|_| descriptor_error())?;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| descriptor_error())?;
    let name = format!(
        ".task-prepare-{}.tmp",
        random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let mut options = cap_std::fs::OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = dir
            .open_with(&name, &options)
            .map_err(|_| descriptor_error())?
            .into_std();
        hagency_store::private::seal_created_file_handle(&file).map_err(|_| descriptor_error())?;
        file.write_all(bytes).map_err(|_| descriptor_error())?;
        file.sync_all().map_err(|_| descriptor_error())?;
        before_publish()?;
        hagency_platform::private_state::publish_new(&dir, &name, INITIALIZER_FILE)
            .map_err(|_| descriptor_error())
    })();
    // A failed publish cannot replace the original name. If publication happened
    // but directory-sync failed, the exact descriptor remains for recovery.
    let _ = dir.remove_file(&name);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn task_prepare_atomic_initializer_never_publishes_partial_or_replaces_pending() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("private");
        hagency_store::private::directory(&state).unwrap();
        let path = state.join(INITIALIZER_FILE);
        assert!(
            persist_new_with(&state, b"original", || {
                assert!(!path.exists());
                Err(descriptor_error())
            })
            .is_err()
        );
        assert!(!path.exists());
        // An interrupted unpublished temporary has no initializer authority.
        hagency_store::private::write_new(&state.join(".task-prepare-orphan.tmp"), b"partial")
            .unwrap();
        persist_new(&state, b"original").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert!(persist_new(&state, b"replacement").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
    }
}
