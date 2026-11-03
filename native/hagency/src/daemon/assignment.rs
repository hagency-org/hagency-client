//! Durable owner-root assignment. Socket liveness never grants local custody.
use super::enrollment::Enrollment;
use std::path::{Path, PathBuf};
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Assignment {
    Local,
    Daemon { home: PathBuf, root: PathBuf },
}
/// Resolve symlink aliases through the nearest existing parent, without
/// creating an owner root merely to inspect its assignment.
fn identity(path: &Path) -> Result<PathBuf, &'static str> {
    if !path.is_absolute() {
        return Err("daemon_owner_root_invalid");
    }
    let mut parent = path;
    let mut suffix = Vec::new();
    loop {
        match parent.canonicalize() {
            Ok(mut real) => {
                for part in suffix.iter().rev() {
                    real.push(part);
                }
                return Ok(real);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                suffix.push(
                    parent
                        .file_name()
                        .ok_or("daemon_owner_root_invalid")?
                        .to_owned(),
                );
                parent = parent.parent().ok_or("daemon_owner_root_invalid")?;
            }
            Err(_) => return Err("daemon_owner_root_unavailable"),
        }
    }
}
pub fn resolve(
    home: &Path,
    origin: &str,
    owner: &str,
    root: &Path,
) -> Result<Assignment, &'static str> {
    let Some(enrolled) = Enrollment::load(home).map_err(|e| e.code())? else {
        return Ok(Assignment::Local);
    };
    if !enrolled.hostable() {
        return Ok(Assignment::Local);
    }
    let adopted = identity(&enrolled.owner_root)?;
    let source = identity(root)?;
    let same_owner = enrolled.owner == owner
        && enrolled.origin.trim_end_matches('/') == origin.trim_end_matches('/');
    if !same_owner {
        return if source == adopted {
            Err("daemon_owner_scope_mismatch")
        } else {
            Ok(Assignment::Local)
        };
    }
    if source != adopted && root.exists() {
        return Err("daemon_owner_root_conflict");
    }
    Ok(Assignment::Daemon {
        home: home.into(),
        root: adopted,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn assignment_survives_missing_socket_and_only_release_allows_local() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("daemon");
        hagency_store::private::directory(&home).unwrap();
        let root = temp.path().join("original");
        std::fs::create_dir(&root).unwrap();
        assert_eq!(
            resolve(&home, "https://s/", "@o:s", &root).unwrap(),
            Assignment::Local
        );
        Enrollment::stage_handover(&home, "https://s/", "@o:s", None, &root).unwrap();
        assert!(matches!(
            resolve(&home, "https://s", "@o:s", &root).unwrap(),
            Assignment::Daemon { .. }
        ));
        assert!(!home.join("control.sock").exists());
        assert!(matches!(
            resolve(&home, "https://s/", "@o:s", &root).unwrap(),
            Assignment::Daemon { .. }
        ));
        std::fs::create_dir(temp.path().join("fresh")).unwrap();
        assert_eq!(
            resolve(&home, "https://s/", "@o:s", &temp.path().join("fresh")).unwrap_err(),
            "daemon_owner_root_conflict"
        );
        assert_eq!(
            resolve(&home, "https://s/", "@other:s", &root).unwrap_err(),
            "daemon_owner_scope_mismatch"
        );
        Enrollment::mark_active(&home).unwrap();
        assert!(matches!(
            resolve(&home, "https://s/", "@o:s", &root).unwrap(),
            Assignment::Daemon { .. }
        ));
        Enrollment::release(&home).unwrap();
        assert_eq!(
            resolve(&home, "https://s/", "@o:s", &root).unwrap(),
            Assignment::Local
        );
    }
    #[cfg(unix)]
    #[test]
    fn root_alias_cannot_bypass_assignment() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("daemon");
        hagency_store::private::directory(&home).unwrap();
        let root = temp.path().join("original");
        std::fs::create_dir(&root).unwrap();
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        Enrollment::stage_handover(&home, "https://s/", "@o:s", None, &root).unwrap();
        assert!(matches!(
            resolve(&home, "https://s/", "@o:s", &alias).unwrap(),
            Assignment::Daemon { .. }
        ));
    }
}
