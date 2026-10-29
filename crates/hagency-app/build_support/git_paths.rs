//! Existing Git metadata inputs for both ordinary checkouts and linked worktrees.
use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git_path(root: &Path, name: &str) -> Option<PathBuf> {
    let value = git_output(root, &["rev-parse", "--git-path", name])?;
    if value.is_empty() {
        return None;
    }
    let path = PathBuf::from(value);
    Some(if path.is_absolute() {
        path
    } else {
        root.join(path)
    })
}

/// Cargo treats a missing rerun-if-changed input as always dirty. Resolve paths
/// through Git rather than assuming `.git` is a directory. When a branch is
/// packed, also watch an existing parent for creation of its future loose ref.
pub fn watch_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let Some(head) = git_path(root, "HEAD").filter(|path| path.is_file()) else {
        return paths; // Source archives legitimately contain no Git metadata.
    };
    paths.push(head);
    // A repaired/retargeted linked worktree may leave the old metadata alive.
    let pointer = root.join(".git");
    if pointer.is_file() {
        paths.push(pointer);
    }
    if let Some(branch) = git_output(root, &["symbolic-ref", "--quiet", "HEAD"]) {
        if let Some(reference) = git_path(root, &branch) {
            if reference.is_file() {
                paths.push(reference);
            } else if let Some(parent) = reference.ancestors().skip(1).find(|path| path.is_dir()) {
                paths.push(parent.to_owned());
            }
        }
        if let Some(packed) = git_path(root, "packed-refs").filter(|path| path.is_file()) {
            paths.push(packed);
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

#[cfg(test)]
mod git_path_tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    struct Fixture(PathBuf);

    // canonicalize uses Windows' extended-length API spelling, but Git's
    // worktree/config arguments need the ordinary drive or UNC spelling.
    // Keep canonicalization (including macOS /private resolution), then change
    // only the Windows verbatim prefix; no file-system target is changed.
    fn git_cli_path(path: &Path) -> PathBuf {
        #[cfg(windows)]
        {
            use std::{ffi::OsString, path::{Component, Prefix}};
            let mut components = path.components();
            if let Some(Component::Prefix(prefix)) = components.next() {
                let ordinary = match prefix.kind() {
                    Prefix::VerbatimDisk(drive) => Some(OsString::from(format!("{}:", char::from(drive)))),
                    Prefix::VerbatimUNC(server, share) => {
                        let mut ordinary = OsString::from(r"\\");
                        ordinary.push(server);
                        ordinary.push(r"\");
                        ordinary.push(share);
                        Some(ordinary)
                    }
                    _ => None,
                };
                if let Some(ordinary) = ordinary {
                    return PathBuf::from(ordinary).join(components.as_path());
                }
            }
        }
        path.to_owned()
    }

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "hagency git input {} {} {}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(git_cli_path(&path.canonicalize().unwrap()))
        }
        fn repo(&self) -> PathBuf {
            let root = self.0.join("repo");
            fs::create_dir(&root).unwrap();
            git(&root, &["init", "--quiet", "--initial-branch=main"]);
            git(&root, &["config", "user.name", "Fixture"]);
            git(&root, &["config", "user.email", "fixture@example.invalid"]);
            git(&root, &["config", "commit.gpgsign", "false"]);
            git(
                &root,
                &[
                    "config",
                    "core.hooksPath",
                    self.0.join("no-hooks").to_str().unwrap(),
                ],
            );
            git(
                &root,
                &["commit", "--quiet", "--allow-empty", "-m", "initial"],
            );
            root
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git").current_dir(root).args(args).output()
            .unwrap_or_else(|error| panic!("git {args:?} could not start in {}: {error}", root.display()));
        assert!(output.status.success(), "git {args:?} failed in {} with {}\nstdout: {}\nstderr: {}",
            root.display(), output.status, String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    #[cfg(windows)]
    #[test]
    fn git_arguments_keep_original_windows_drive_and_unc_paths() {
        for (api, cli) in [
            (r"\\?\D:\hagency git input\linked with spaces", r"D:\hagency git input\linked with spaces"),
            (r"\\?\UNC\server\share\hagency git input", r"\\server\share\hagency git input"),
            (r"D:\already ordinary\path", r"D:\already ordinary\path"),
        ] {
            assert_eq!(git_cli_path(Path::new(api)), PathBuf::from(cli));
        }
    }
    fn assert_existing(paths: &[PathBuf]) {
        assert!(!paths.is_empty());
        assert!(
            paths.iter().all(|path| path.exists()),
            "missing Cargo input: {paths:?}"
        );
    }

    #[test]
    fn ordinary_git_inputs_follow_loose_and_packed_branch_updates() {
        let fixture = Fixture::new();
        let root = fixture.repo();
        let paths = watch_paths(&root);
        assert_existing(&paths);
        assert!(paths.contains(&root.join(".git/HEAD")));
        assert!(paths.contains(&root.join(".git/refs/heads/main")));
        git(&root, &["pack-refs", "--all", "--prune"]);
        assert!(!root.join(".git/refs/heads/main").exists());
        let packed = watch_paths(&root);
        assert_existing(&packed);
        assert!(packed.contains(&root.join(".git/packed-refs")));
        assert!(
            packed
                .iter()
                .any(|path| path.is_dir() && root.join(".git/refs/heads/main").starts_with(path))
        );
        let old = git(&root, &["rev-parse", "HEAD"]);
        git(
            &root,
            &["commit", "--quiet", "--allow-empty", "-m", "updated"],
        );
        assert_ne!(old, git(&root, &["rev-parse", "HEAD"]));
        assert!(watch_paths(&root).contains(&root.join(".git/refs/heads/main")));
    }

    #[test]
    fn linked_worktree_inputs_watch_real_head_and_common_ref_including_detach() {
        let fixture = Fixture::new();
        let root = fixture.repo();
        let linked = fixture.0.join("linked with spaces");
        git(
            &root,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "linked",
                linked.to_str().unwrap(),
            ],
        );
        assert!(linked.join(".git").is_file());
        assert!(!linked.join(".git/HEAD").exists());
        let paths = watch_paths(&linked);
        assert_existing(&paths);
        let real_head = git_path(&linked, "HEAD").unwrap();
        assert!(paths.contains(&real_head));
        assert!(paths.iter().any(|path| fs::canonicalize(path).ok()
            == fs::canonicalize(root.join(".git/refs/heads/linked")).ok()));
        let old = fs::read(&real_head).unwrap();
        git(&linked, &["checkout", "--quiet", "--detach"]);
        assert_ne!(old, fs::read(&real_head).unwrap());
        let detached = watch_paths(&linked);
        assert_eq!(detached.len(), 2);
        assert!(detached.contains(&real_head) && detached.contains(&linked.join(".git")));
        let old = fs::read(&real_head).unwrap();
        git(
            &linked,
            &["commit", "--quiet", "--allow-empty", "-m", "detached"],
        );
        assert_ne!(old, fs::read(&real_head).unwrap());
        assert_eq!(watch_paths(&linked), detached);
    }

    #[test]
    fn source_archives_emit_no_missing_git_watch_inputs() {
        let fixture = Fixture::new();
        assert!(watch_paths(&fixture.0).is_empty());
    }
}
