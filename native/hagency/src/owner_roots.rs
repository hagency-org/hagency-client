//! One owner root per account per computer (ADR-192).
//!
//! For each (normalized server origin, owner) the OS user has exactly one
//! owner root. Its path is recorded once, private and create-new, under the
//! OS-owner resource home:
//! `~/.hagency-owner-resources/owner-roots/<sha256(json([origin, owner]))>.json`.
//! The first writer wins; only explicit removal clears a record. Roots are
//! never moved, because the native resume record's scope and the provider
//! credential references are bound to the path.
//!
//! Resolution order:
//! 1. the registered root;
//! 2. otherwise the caller's existing root (Desktop's), registered in place;
//! 3. otherwise `~/.hagency-owner-resources/owners/<key>`, registered.
//!
//! Desktop and the owner daemon resolve roots only through this registry.
//! Unix only: other platforms keep the caller's in-process root unchanged.
use std::path::{Path, PathBuf};

/// Directory of root records inside the registry base.
pub const RECORDS: &str = "owner-roots";
/// Directory of canonical roots created for owners without an existing root.
pub const OWNERS: &str = "owners";

#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
pub enum RootError {
    #[error("the server origin or owner is invalid")]
    Identity,
    #[error("an owner root must be an absolute owner-client directory without symbolic links")]
    Root,
    #[error("the owner-root record is invalid")]
    Record,
    #[error("the owner-root registry is unavailable")]
    Unavailable,
    #[error("this platform has no owner-root registry")]
    Unsupported,
    #[error("this account has no owner root on this computer; set up hosting from Hagency Desktop")]
    NotRegistered,
}

impl RootError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Identity => "owner_identity_invalid",
            Self::Root => "owner_root_invalid",
            Self::Record => "owner_root_record_invalid",
            Self::Unavailable => "owner_root_registry_unavailable",
            Self::Unsupported => "owner_root_registry_unsupported",
            Self::NotRegistered => "owner_root_not_registered",
        }
    }
}

/// Origin equivalence for owner roots and daemon enrollment: a trailing `/`
/// is not scope, and scheme and host compare case-insensitively. A parseable
/// URL is compared in its serialized form, which also folds a default port.
pub fn normalize_origin(origin: &str) -> String {
    if let Ok(url) = reqwest::Url::parse(origin)
        && url.has_host()
    {
        return url.as_str().trim_end_matches('/').to_owned();
    }
    let trimmed = origin.trim_end_matches('/');
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return trimmed.to_owned();
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let (user, host) = match authority.rsplit_once('@') {
        Some((user, host)) => (&authority[..=user.len()], host),
        None => ("", authority),
    };
    format!(
        "{}://{user}{}{tail}",
        scheme.to_ascii_lowercase(),
        host.to_ascii_lowercase()
    )
}

/// Registry key: hex SHA-256 of the JSON array `[normalized origin, owner]`.
pub fn key(origin: &str, owner: &str) -> String {
    use sha2::{Digest, Sha256};
    let identity =
        serde_json::to_vec(&[normalize_origin(origin).as_str(), owner]).unwrap_or_default();
    format!("{:x}", Sha256::digest(identity))
}

fn check_identity(origin: &str, owner: &str) -> Result<(), RootError> {
    if crate::server_admission::normalize_origin(origin).is_err()
        || !crate::daemon::protocol::valid_owner_id(owner)
    {
        return Err(RootError::Identity);
    }
    Ok(())
}

/// Resolve symlink aliases through the nearest existing ancestor, without
/// creating anything. `InvalidInput` for a relative or unnamed path.
pub(crate) fn real_path(path: &Path) -> std::io::Result<PathBuf> {
    let invalid = || std::io::Error::from(std::io::ErrorKind::InvalidInput);
    if !path.is_absolute() {
        return Err(invalid());
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
                suffix.push(parent.file_name().ok_or_else(invalid)?.to_owned());
                parent = parent.parent().ok_or_else(invalid)?;
            }
            Err(e) => return Err(e),
        }
    }
}

/// The registered root for an owner, with the machine's registry. Read-only.
pub fn lookup(origin: &str, owner: &str) -> Result<Option<PathBuf>, RootError> {
    OwnerRoots::system()?.lookup(origin, owner)
}

/// The one owner root for (origin, owner) on this computer, registering it
/// when this is the first resolution. `existing_root` is the caller's current
/// root (Desktop's), used only when nothing is registered yet and it exists.
pub fn resolve(
    origin: &str,
    owner: &str,
    existing_root: Option<&Path>,
) -> Result<PathBuf, RootError> {
    OwnerRoots::system()?.resolve(origin, owner, existing_root)
}

/// The root registry under one base directory. Production uses
/// [`OwnerRoots::system`]; tests pass their own base to [`OwnerRoots::at`].
#[derive(Clone, Debug)]
pub struct OwnerRoots {
    base: PathBuf,
}

impl OwnerRoots {
    /// `~/.hagency-owner-resources` of the OS account (not `$HOME`), the same
    /// directory as the resource ledger. A test-support build with an explicit
    /// isolated installation uses that installation's ledger directory.
    pub fn system() -> Result<Self, RootError> {
        #[cfg(all(feature = "resource-adoption", any(test, feature = "test-support")))]
        if let Some(root) = crate::native_owner::resource_activation_fixture_root() {
            return Ok(Self {
                base: root.join("canonical"),
            });
        }
        // Unit tests never read or write the OS owner's registry.
        #[cfg(test)]
        return Err(RootError::Unavailable);
        #[cfg(not(test))]
        {
            let home = hagency_platform::owner_home().map_err(|_| RootError::Unavailable)?;
            Ok(Self {
                base: home.join(".hagency-owner-resources"),
            })
        }
    }

    /// A registry rooted at an explicit absolute base directory.
    pub fn at(base: impl Into<PathBuf>) -> Result<Self, RootError> {
        let base = base.into();
        if !base.is_absolute() {
            return Err(RootError::Unavailable);
        }
        Ok(Self { base })
    }

    pub fn base(&self) -> &Path {
        &self.base
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use serde::{Deserialize, Serialize};

    const VERSION: u8 = 1;
    const LIMIT: usize = 8 * 1024;

    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Record {
        version: u8,
        origin: String,
        owner: String,
        root: PathBuf,
    }

    /// Absolute, no symbolic links or relative components, and a directory
    /// when it exists. Not-yet-created canonical roots are valid.
    fn check_root(root: &Path) -> Result<(), RootError> {
        let real = match real_path(root) {
            Ok(real) => real,
            Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => return Err(RootError::Root),
            Err(_) => return Err(RootError::Unavailable),
        };
        if real.as_os_str() != root.as_os_str() {
            return Err(RootError::Root);
        }
        match std::fs::symlink_metadata(root) {
            Ok(meta) if !meta.is_dir() => Err(RootError::Root),
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(RootError::Unavailable),
        }
    }

    /// An existing owner-client directory, canonicalized. `None` when
    /// nothing exists at the path.
    fn existing(path: &Path) -> Result<Option<PathBuf>, RootError> {
        if !path.is_absolute() {
            return Err(RootError::Root);
        }
        match std::fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(RootError::Unavailable),
            Ok(_) => {}
        }
        let real = path.canonicalize().map_err(|_| RootError::Root)?;
        if !std::fs::metadata(&real).is_ok_and(|m| m.is_dir())
            || !crate::owner_host::is_owner_root_format(&real)
        {
            return Err(RootError::Root);
        }
        Ok(Some(real))
    }

    fn private_directory(path: &Path) -> Result<(), RootError> {
        hagency_store::private::directory(path).map_err(|_| RootError::Unavailable)
    }

    /// Place `from` at `to` only if `to` does not exist, atomically.
    fn place_new(from: &Path, to: &Path) -> std::io::Result<()> {
        #[cfg(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "linux",
            target_os = "android"
        ))]
        {
            use rustix::{
                fs::{CWD, RenameFlags, renameat_with},
                io::Errno,
            };
            match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
                Ok(()) => return Ok(()),
                // Filesystems without an exclusive rename fall back to a link.
                Err(e) if e == Errno::INVAL || e == Errno::NOSYS || e == Errno::NOTSUP => {}
                Err(e) => return Err(e.into()),
            }
        }
        std::fs::hard_link(from, to)
    }

    impl OwnerRoots {
        fn record_path(&self, origin: &str, owner: &str) -> PathBuf {
            self.base
                .join(RECORDS)
                .join(format!("{}.json", key(origin, owner)))
        }

        /// The registered root, validated on every read. Creates nothing.
        pub fn lookup(&self, origin: &str, owner: &str) -> Result<Option<PathBuf>, RootError> {
            check_identity(origin, owner)?;
            let raw = crate::daemon::read_private(&self.record_path(origin, owner), LIMIT)
                .map_err(|_| RootError::Record)?;
            let Some(raw) = raw else { return Ok(None) };
            let record: Record = serde_json::from_slice(&raw).map_err(|_| RootError::Record)?;
            if record.version != VERSION
                || record.origin != normalize_origin(origin)
                || record.owner != owner
            {
                return Err(RootError::Record);
            }
            check_root(&record.root).map_err(|e| match e {
                RootError::Unavailable => e,
                _ => RootError::Record,
            })?;
            Ok(Some(record.root))
        }

        /// See the module documentation for the resolution order. Returns the
        /// root another resolver registered first if it wins a race.
        pub fn resolve(
            &self,
            origin: &str,
            owner: &str,
            existing_root: Option<&Path>,
        ) -> Result<PathBuf, RootError> {
            if let Some(root) = self.lookup(origin, owner)? {
                return Ok(root);
            }
            let root = match existing_root.map(existing).transpose()?.flatten() {
                Some(root) => root,
                None => {
                    private_directory(&self.base)?;
                    let base = self
                        .base
                        .canonicalize()
                        .map_err(|_| RootError::Unavailable)?;
                    private_directory(&base.join(OWNERS))?;
                    base.join(OWNERS).join(key(origin, owner))
                }
            };
            self.register(origin, owner, &root)
        }

        fn register(&self, origin: &str, owner: &str, root: &Path) -> Result<PathBuf, RootError> {
            check_root(root)?;
            let record = Record {
                version: VERSION,
                origin: normalize_origin(origin),
                owner: owner.into(),
                root: root.into(),
            };
            let bytes = serde_json::to_vec(&record).map_err(|_| RootError::Root)?;
            let directory = self.base.join(RECORDS);
            private_directory(&self.base)?;
            private_directory(&directory)?;
            let mut suffix = [0u8; 8];
            getrandom::fill(&mut suffix).map_err(|_| RootError::Unavailable)?;
            let temporary = directory.join(format!(
                ".{}.tmp",
                suffix
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            ));
            hagency_store::private::write_new(&temporary, &bytes)
                .map_err(|_| RootError::Unavailable)?;
            let placed = place_new(&temporary, &self.record_path(origin, owner));
            // Gone after a rename; the source of a fallback link otherwise.
            let _ = std::fs::remove_file(&temporary);
            match placed {
                Ok(()) => {
                    std::fs::File::open(&directory)
                        .and_then(|d| d.sync_all())
                        .map_err(|_| RootError::Unavailable)?;
                    Ok(root.into())
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    self.lookup(origin, owner)?.ok_or(RootError::Record)
                }
                Err(_) => Err(RootError::Unavailable),
            }
        }
    }
}

#[cfg(not(unix))]
impl OwnerRoots {
    /// No registry on this platform yet.
    pub fn lookup(&self, origin: &str, owner: &str) -> Result<Option<PathBuf>, RootError> {
        check_identity(origin, owner)?;
        Ok(None)
    }

    /// The caller's in-process root, unchanged.
    pub fn resolve(
        &self,
        origin: &str,
        owner: &str,
        existing_root: Option<&Path>,
    ) -> Result<PathBuf, RootError> {
        check_identity(origin, owner)?;
        existing_root
            .map(Path::to_path_buf)
            .ok_or(RootError::Unsupported)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    const OWNER: &str = "@owner:s.example";

    fn registry() -> (tempfile::TempDir, OwnerRoots) {
        let temp = tempfile::tempdir().unwrap();
        let roots = OwnerRoots::at(temp.path().join("resources")).unwrap();
        (temp, roots)
    }

    fn desktop_root(temp: &tempfile::TempDir, name: &str) -> PathBuf {
        let root = temp.path().join(name);
        crate::owner_host::initialize(&root).unwrap();
        root.canonicalize().unwrap()
    }

    #[test]
    fn origins_normalize_the_trailing_slash_and_case_of_scheme_and_host() {
        for (origin, expected) in [
            ("https://s.example", "https://s.example"),
            ("https://s.example/", "https://s.example"),
            ("HTTPS://S.Example/", "https://s.example"),
            ("https://s.example:443/", "https://s.example"),
            ("http://127.0.0.1:8008/", "http://127.0.0.1:8008"),
            ("not an origin/", "not an origin"),
        ] {
            assert_eq!(normalize_origin(origin), expected, "{origin}");
        }
        assert_eq!(
            key("https://s.example", OWNER),
            key("https://S.example/", OWNER)
        );
        assert_ne!(
            key("https://s.example", OWNER),
            key("https://t.example", OWNER)
        );
        assert_ne!(
            key("https://s.example", OWNER),
            key("https://s.example", "@other:s.example")
        );
        assert!(crate::daemon::same_origin(
            "https://S.example/",
            "https://s.example"
        ));
        assert!(!crate::daemon::same_origin(
            "https://s.example",
            "https://t.example"
        ));
    }

    #[test]
    fn the_default_root_is_canonical_and_registered_once_for_both_slash_forms() {
        let (_temp, roots) = registry();
        assert_eq!(roots.lookup("https://s.example", OWNER).unwrap(), None);
        let root = roots.resolve("https://s.example", OWNER, None).unwrap();
        let base = roots.base().canonicalize().unwrap();
        assert_eq!(
            root,
            base.join(OWNERS).join(key("https://s.example", OWNER))
        );
        // Registration names the root; the owner host creates it on first open.
        assert!(!root.exists());
        assert_eq!(
            roots.resolve("https://s.example/", OWNER, None).unwrap(),
            root
        );
        assert_eq!(
            roots.lookup("https://s.example/", OWNER).unwrap(),
            Some(root.clone())
        );
        let record = base
            .join(RECORDS)
            .join(format!("{}.json", key("https://s.example", OWNER)));
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(&record).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::metadata(base.join(RECORDS)).unwrap().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(base.join(OWNERS)).unwrap().mode() & 0o777,
            0o700
        );
        // Another owner on the same server gets its own root.
        assert_ne!(
            roots
                .resolve("https://s.example", "@other:s.example", None)
                .unwrap(),
            root
        );
    }

    #[test]
    fn an_existing_desktop_root_is_registered_in_place_and_wins_afterwards() {
        let (temp, roots) = registry();
        let desktop = desktop_root(&temp, "desktop-root");
        assert_eq!(
            roots
                .resolve("https://s.example/", OWNER, Some(&desktop))
                .unwrap(),
            desktop
        );
        // Later resolutions return it, with or without another existing root.
        let other = desktop_root(&temp, "other-desktop-profile");
        assert_eq!(
            roots
                .resolve("https://s.example", OWNER, Some(&other))
                .unwrap(),
            desktop
        );
        assert_eq!(
            roots.resolve("https://s.example", OWNER, None).unwrap(),
            desktop
        );
        assert!(
            !roots.base().join(OWNERS).exists(),
            "no canonical root was created"
        );
        // A missing existing root falls through to the canonical default.
        let missing = temp.path().join("never-created");
        let fresh = roots
            .resolve("https://t.example", OWNER, Some(&missing))
            .unwrap();
        assert!(fresh.starts_with(roots.base().canonicalize().unwrap().join(OWNERS)));
    }

    #[test]
    fn the_first_writer_wins() {
        let (temp, roots) = registry();
        let first = desktop_root(&temp, "first");
        let second = desktop_root(&temp, "second");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let winners: Vec<PathBuf> = (0..8)
            .map(|n| {
                let (roots, barrier) = (roots.clone(), barrier.clone());
                let candidate = if n % 2 == 0 {
                    first.clone()
                } else {
                    second.clone()
                };
                std::thread::spawn(move || {
                    barrier.wait();
                    roots
                        .resolve("https://s.example", OWNER, Some(&candidate))
                        .unwrap()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect();
        assert!(winners.iter().all(|w| *w == winners[0]), "{winners:?}");
        assert!(winners[0] == first || winners[0] == second);
        let records: Vec<_> = std::fs::read_dir(roots.base().join(RECORDS))
            .unwrap()
            .collect();
        assert_eq!(records.len(), 1, "one record and no temporary files remain");
    }

    #[test]
    fn records_are_validated_on_read() {
        let (temp, roots) = registry();
        let desktop = desktop_root(&temp, "desktop-root");
        roots
            .resolve("https://s.example", OWNER, Some(&desktop))
            .unwrap();
        let record = roots
            .base()
            .join(RECORDS)
            .join(format!("{}.json", key("https://s.example", OWNER)));
        let write = |value: serde_json::Value| {
            hagency_store::private::replace(&record, &serde_json::to_vec(&value).unwrap()).unwrap()
        };
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&desktop, &alias).unwrap();
        for bad in [
            serde_json::json!({"version":1,"origin":"https://s.example","owner":OWNER,"root":"relative/root"}),
            serde_json::json!({"version":1,"origin":"https://s.example","owner":OWNER,"root":alias}),
            serde_json::json!({"version":1,"origin":"https://s.example","owner":OWNER,"root":desktop.join("..").join("desktop-root")}),
            serde_json::json!({"version":1,"origin":"https://t.example","owner":OWNER,"root":desktop}),
            serde_json::json!({"version":1,"origin":"https://s.example","owner":"@other:s.example","root":desktop}),
            serde_json::json!({"version":2,"origin":"https://s.example","owner":OWNER,"root":desktop}),
            serde_json::json!({"version":1,"origin":"https://s.example","owner":OWNER,"root":desktop,"extra":true}),
        ] {
            write(bad.clone());
            assert_eq!(
                roots.lookup("https://s.example", OWNER),
                Err(RootError::Record),
                "{bad}"
            );
            assert_eq!(
                roots.resolve("https://s.example", OWNER, None),
                Err(RootError::Record),
                "{bad}"
            );
        }
        // A non-private record is refused.
        write(
            serde_json::json!({"version":1,"origin":"https://s.example","owner":OWNER,"root":desktop}),
        );
        assert_eq!(
            roots.lookup("https://s.example", OWNER).unwrap(),
            Some(desktop.clone())
        );
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            roots.lookup("https://s.example", OWNER),
            Err(RootError::Record)
        );
    }

    #[test]
    fn only_owner_client_directories_can_be_registered_in_place() {
        let (temp, roots) = registry();
        let file = temp.path().join("file");
        std::fs::write(&file, b"x").unwrap();
        let foreign = temp.path().join("foreign");
        std::fs::create_dir(&foreign).unwrap();
        std::fs::write(foreign.join("notes.txt"), b"x").unwrap();
        for bad in [file.as_path(), foreign.as_path(), Path::new("relative")] {
            assert_eq!(
                roots.resolve("https://s.example", OWNER, Some(bad)),
                Err(RootError::Root),
                "{bad:?}"
            );
        }
        assert_eq!(
            roots.lookup("https://s.example", OWNER).unwrap(),
            None,
            "nothing was registered"
        );
        // An empty directory is a root the owner host has not initialized yet.
        let empty = temp.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert_eq!(
            roots
                .resolve("https://s.example", OWNER, Some(&empty))
                .unwrap(),
            empty.canonicalize().unwrap()
        );
        for (origin, owner) in [
            ("ftp://s.example", OWNER),
            ("https://s.example", "owner"),
            ("https://s.example/path", OWNER),
        ] {
            assert_eq!(
                roots.resolve(origin, owner, None),
                Err(RootError::Identity),
                "{origin} {owner}"
            );
        }
    }

    #[test]
    fn unit_tests_never_select_the_os_owner_registry() {
        // Another test may select an isolated installation for the process.
        match OwnerRoots::system() {
            Err(error) => assert_eq!(error, RootError::Unavailable),
            #[cfg(feature = "resource-adoption")]
            Ok(roots) => assert_eq!(
                Some(roots.base().to_path_buf()),
                crate::native_owner::resource_activation_fixture_root()
                    .map(|r| r.join("canonical"))
            ),
            #[cfg(not(feature = "resource-adoption"))]
            Ok(roots) => panic!("unit tests selected {:?}", roots.base()),
        }
        assert!(OwnerRoots::at("relative").is_err());
    }
}
