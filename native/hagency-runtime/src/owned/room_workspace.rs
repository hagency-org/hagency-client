use crate::codex::session::Error;
use hagency_platform::private_state;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The host supplies an Agent root and the verified work item's full room ID.
/// Hash the ID verbatim: room names, thread IDs and path syntax are not keys.
/// Leave legacy files in the Agent root; do not copy them into any room.
pub(super) fn open(root: &Path, room: &str) -> Result<PathBuf, Error> {
    if !room.starts_with('!') || room.len() < 2 || room.len() > 255
        || room.chars().any(char::is_control)
        || !root.is_absolute() || root.canonicalize().ok().as_deref() != Some(root) {
        return Err(Error::Scope);
    }
    let rooms = root.join("rooms");
    private_state::directory(&rooms).map_err(|_| Error::Settings)?;
    if rooms.canonicalize().ok().as_ref() != Some(&rooms) {
        return Err(Error::Scope);
    }
    let directory = rooms.join(format!("room_{:x}", Sha256::digest(room.as_bytes())));
    private_state::directory(&directory).map_err(|_| Error::Settings)?;
    if directory.canonicalize().ok().as_ref() != Some(&directory) {
        return Err(Error::Scope);
    }
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooms_and_agents_keep_distinct_persistent_files_without_migrating_legacy_files() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let first = base.join("agent_one");
        let second = base.join("agent_two");
        private_state::directory(&first).unwrap();
        private_state::directory(&second).unwrap();
        std::fs::write(first.join("legacy.txt"), "legacy").unwrap();
        let dm = open(&first, "!dm:river.ominix.io:19443").unwrap();
        let group = open(&first, "!group:river.ominix.io:19443").unwrap();
        let other_agent = open(&second, "!dm:river.ominix.io:19443").unwrap();
        assert_ne!(dm, group);
        assert_ne!(dm, other_agent);
        std::fs::write(dm.join("result.txt"), "dm").unwrap();
        std::fs::write(group.join("result.txt"), "group").unwrap();
        assert_eq!(open(&first, "!dm:river.ominix.io:19443").unwrap(), dm);
        assert_eq!(std::fs::read(dm.join("result.txt")).unwrap(), b"dm");
        assert_eq!(std::fs::read(group.join("result.txt")).unwrap(), b"group");
        assert!(!other_agent.join("result.txt").exists());
        assert!(!dm.join("legacy.txt").exists());
        assert_eq!(std::fs::read(first.join("legacy.txt")).unwrap(), b"legacy");
        assert_ne!(open(&first, "!dm:other-server").unwrap(), dm);
        let unusual = open(&first, "!../../escape:server").unwrap();
        assert_eq!(unusual.parent(), Some(first.join("rooms").as_path()));
        for invalid in ["", "!", "../escape", "!room\n:server"] {
            assert!(open(&first, invalid).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn room_workspace_rejects_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("agent");
        private_state::directory(&root).unwrap();
        std::os::unix::fs::symlink(&base, root.join("rooms")).unwrap();
        assert!(open(&root, "!dm:server").is_err());
        std::fs::remove_file(root.join("rooms")).unwrap();
        let room = open(&root, "!dm:server").unwrap();
        std::fs::remove_dir(&room).unwrap();
        std::os::unix::fs::symlink(&base, &room).unwrap();
        assert!(open(&root, "!dm:server").is_err());
    }
}
