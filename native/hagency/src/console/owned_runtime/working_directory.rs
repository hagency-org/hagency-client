//! Room-specific working directories without changing execution permissions.
use super::{Profile, RuntimeError, Scope};
use hagency_agent_local::room_files::{self, Workspace};

pub(super) fn for_turn(
    configured: &Profile,
    owner: &str,
    scope: &Scope,
    host_files: bool,
) -> Result<(Profile, Option<Workspace>), RuntimeError> {
    let mut profile = configured.clone();
    let workspace = if host_files {
        // Preserve the existing explicitly granted file-capability namespace.
        let directory = profile.home.parent().ok_or(RuntimeError::Profile)?;
        let workspace =
            Workspace::open(directory, owner, scope).map_err(|_| RuntimeError::Profile)?;
        profile.cwd = workspace.canonical_directory().to_path_buf();
        Some(workspace)
    } else {
        // A private child of the owner's selected base; no file tools granted.
        profile.cwd = room_files::chat_directory(&profile.cwd, owner, scope)
            .map_err(|_| RuntimeError::Profile)?;
        None
    };
    Ok((profile, workspace))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chat_only_rooms_keep_selected_base_and_never_enable_file_tools() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        std::fs::write(base.join("existing-project.txt"), "preserved").unwrap();
        let profile = Profile {
            executable: base.join("provider"),
            home: base.join("provider-home"),
            codex_home: base.join("codex-home"),
            cwd: base.clone(),
            model: "fixture".into(),
            effort: "low".into(),
            shared_auth: false,
            web_search: false,
        };
        let scope = Scope {
            agent: "agent".into(),
            binding: "dm".into(),
            room: "!dm:test".into(),
            requester: "@owner:test".into(),
            thread: "!dm:test".into(),
        };
        let (dm, files) = for_turn(&profile, "@owner:test", &scope, false).unwrap();
        assert!(files.is_none());
        assert!(dm.cwd.starts_with(&base));
        assert_ne!(dm.cwd, base);
        let mut group = scope.clone();
        group.binding = "group".into();
        group.room = "!group:test".into();
        let (group, files) = for_turn(&profile, "@owner:test", &group, false).unwrap();
        assert!(files.is_none());
        assert_ne!(dm.cwd, group.cwd);
        assert_eq!(
            dm.cwd,
            for_turn(&profile, "@owner:test", &scope, false)
                .unwrap()
                .0
                .cwd
        );
        assert_eq!(profile.cwd, base);
        assert_eq!(
            std::fs::read_to_string(base.join("existing-project.txt")).unwrap(),
            "preserved"
        );
        assert!(!dm.web_search);
    }
}
