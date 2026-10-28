//! Cooperative OS-owner writer custody. No resource ledger, import, authority
//! grant. Refusal publication exists only in the disabled adoption feature.
use crate::private_state as private;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::fs::{Dir, OpenOptions};
use serde::Deserialize;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

const LOCK: &str = "resource-authority.lock";
const REFUSAL: &str = "legacy-resource-refusal.json";
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("filesystem error")]
    Io(#[from] std::io::Error),
    #[error("owner-private state required")]
    Private,
    #[error("legacy resource authority is retired")]
    Refused,
    #[error("installation refusal state requires inspected recovery")]
    Quarantined,
    #[error("installation resource writer is busy")]
    Locked,
    #[error("migration writer coverage is unavailable on this platform")]
    Unavailable,
    #[error("test installation root is missing or cannot be changed")]
    TestRoot,
}
impl From<private::Error> for Error {
    fn from(error: private::Error) -> Self {
        match error {
            private::Error::Io(error) => Self::Io(error),
            private::Error::Private => Self::Private,
            private::Error::PlatformUnavailable => Self::Unavailable,
        }
    }
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
/// Supported desktop Unix implementation. This is not runtime test evidence.
/// Mobile passwd namespaces and Windows lifetime startup are not enabled.
pub const fn supported_legacy_guard_platform() -> bool {
    cfg!(any(
        target_os = "macos",
        all(target_os = "linux", not(target_env = "ohos"))
    ))
}
/// Current runtime qualification for actual adoption exclusion. Linux shares
/// the implementation but has not run this checkpoint's runtime acceptance.
pub const fn adoption_runtime_qualified_platform() -> bool {
    cfg!(target_os = "macos")
}
fn root() -> Result<PathBuf, Error> {
    #[cfg(feature = "test-support")]
    {
        test_support::selected_root()
    }
    #[cfg(not(feature = "test-support"))]
    {
        if !supported_legacy_guard_platform() {
            return Err(Error::Unavailable);
        }
        Ok(crate::owner_home()?.join(".hagency-owner-resources"))
    }
}
/// A disk refusal is only evidence that legacy capacity writers must stop.
/// It cannot prove source fencing, successful import, capacity, or profile ACLs.
#[derive(Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Refusal {
    version: u16,
    registry_id: String,
    migration_id: String,
    inventory_digest: String,
}

struct Files {
    root: PathBuf,
    directory: Dir,
    lock: File,
}
impl Files {
    fn open(root: &Path) -> Result<Self, Error> {
        if !root.is_absolute() {
            return Err(Error::Private);
        }
        private::directory(root)?;
        let parent = Dir::open_ambient_dir(
            root.parent().ok_or(Error::Private)?,
            cap_std::ambient_authority(),
        )?;
        let directory = parent.open_dir_nofollow(root.file_name().ok_or(Error::Private)?)?;
        private::check_handle(&directory.try_clone()?.into_std_file())?;
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .follow(FollowSymlinks::No)
            .nonblock(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = match directory.open_with(LOCK, options.clone().create_new(true)) {
            Ok(file) => {
                let file = file.into_std();
                private::seal_created_file_handle(&file)?;
                file.sync_all()?;
                private::sync_directory(&directory)?;
                file
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                directory.open_with(LOCK, &options)?.into_std()
            }
            Err(error) => return Err(error.into()),
        };
        private::check_handle(&lock)?;
        if !lock.metadata()?.is_file() || lock.metadata()?.len() != 0 {
            return Err(Error::Private);
        }
        let files = Self {
            root: root.into(),
            directory,
            lock,
        };
        files.check()?;
        Ok(files)
    }
    fn check(&self) -> Result<(), Error> {
        let parent = Dir::open_ambient_dir(
            self.root.parent().ok_or(Error::Private)?,
            cap_std::ambient_authority(),
        )?;
        let current = parent.open_dir_nofollow(self.root.file_name().ok_or(Error::Private)?)?;
        if !crate::same_directory(
            &self.directory.try_clone()?.into_std_file(),
            &current.try_clone()?.into_std_file(),
        )? {
            return Err(Error::Private);
        }
        private::check_handle(&current.try_clone()?.into_std_file())?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No).nonblock(true);
        let current_lock = current.open_with(LOCK, &options)?.into_std();
        private::check_handle(&current_lock)?;
        if !crate::same_file(&self.lock, &current_lock)? {
            return Err(Error::Private);
        }
        Ok(())
    }
    fn refusal(&self) -> Result<Option<Refusal>, Error> {
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No).nonblock(true);
        let file = match self.directory.open_with(REFUSAL, &options) {
            Ok(file) => file.into_std(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        private::check_handle(&file)?;
        if !file.metadata()?.is_file() || file.metadata()?.len() > 4096 {
            return Err(Error::Quarantined);
        }
        let mut bytes = Vec::new();
        file.take(4097).read_to_end(&mut bytes)?;
        if bytes.len() > 4096 {
            return Err(Error::Quarantined);
        }
        let refusal: Refusal = serde_json::from_slice(&bytes).map_err(|_| Error::Quarantined)?;
        if refusal.version != 1
            || !digest(&refusal.registry_id)
            || !digest(&refusal.inventory_digest)
            || !identifier(&refusal.migration_id)
        {
            return Err(Error::Quarantined);
        }
        Ok(Some(refusal))
    }
}

/// A shared lock retained until the actual legacy connection/effect is gone.
/// There is no expiry: native authorization remains an independent constraint.
pub struct LegacyWriterGuard {
    _files: Files,
}
impl LegacyWriterGuard {
    pub fn acquire() -> Result<Self, Error> {
        Self::at(&root()?)
    }
    fn at(root: &Path) -> Result<Self, Error> {
        let files = Files::open(root)?;
        files.lock.try_lock_shared().map_err(|_| Error::Locked)?;
        files.check()?;
        if files.refusal()?.is_some() {
            return Err(Error::Refused);
        }
        Ok(Self { _files: files })
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn isolated_for_tests(root: &Path) -> Result<Self, Error> {
        Self::at(root)
    }
}
/// Explicit startup compatibility, never an assertion of installation coverage.
pub enum LegacyWriterAdmission {
    Covered(LegacyWriterGuard),
    MigrationUnavailable,
}
impl LegacyWriterAdmission {
    pub fn acquire_for_legacy_startup() -> Result<Self, Error> {
        if !supported_legacy_guard_platform() && !cfg!(feature = "test-support") {
            return Ok(Self::MigrationUnavailable);
        }
        LegacyWriterGuard::acquire().map(Self::Covered)
    }
    pub fn is_covered(&self) -> bool {
        matches!(self, Self::Covered(_))
    }
}
/// Raw exclusive file custody only. The caller must separately enforce native
/// permission, finite expiry and registry identity.
pub struct ExclusiveWriterGuard {
    files: Files,
    refusal_registry: Option<String>,
}
impl ExclusiveWriterGuard {
    /// Irreversible refusal only; never import or access authority. The native
    /// finite lease owns the separate approval/expiry checks around this call.
    #[cfg(feature = "resource-adoption")]
    pub fn publish_refusal(
        &self,
        registry_id: &str,
        migration_id: &str,
        inventory_digest: &str,
    ) -> Result<(), Error> {
        use std::io::Write;
        self.check()?;
        if !digest(registry_id) || !identifier(migration_id) || !digest(inventory_digest) {
            return Err(Error::Quarantined);
        }
        let expected = Refusal {
            version: 1,
            registry_id: registry_id.into(),
            migration_id: migration_id.into(),
            inventory_digest: inventory_digest.into(),
        };
        if let Some(old) = self.files.refusal()? {
            return if old == expected {
                Ok(())
            } else {
                Err(Error::Quarantined)
            };
        }
        let bytes = serde_json::to_vec(&expected).map_err(|_| Error::Quarantined)?;
        let temporary = "legacy-resource-refusal.pending";
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No)
            .nonblock(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match self.files.directory.open_with(temporary, &options) {
            Ok(file) => {
                let mut file = file.into_std();
                private::seal_created_file_handle(&file)?;
                file.write_all(&bytes)?;
                file.sync_all()?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let mut options = OpenOptions::new();
                options.read(true).follow(FollowSymlinks::No).nonblock(true);
                let mut file = self
                    .files
                    .directory
                    .open_with(temporary, &options)?
                    .into_std();
                private::check_handle(&file)?;
                if !file.metadata()?.is_file() || file.metadata()?.len() > 4096 {
                    return Err(Error::Quarantined);
                }
                let mut old = Vec::new();
                file.read_to_end(&mut old)?;
                if !bytes.starts_with(&old) {
                    return Err(Error::Quarantined);
                }
                if old.len() != bytes.len() {
                    use std::io::{Seek, SeekFrom};
                    let mut writable = OpenOptions::new();
                    writable
                        .read(true)
                        .write(true)
                        .follow(FollowSymlinks::No)
                        .nonblock(true);
                    let mut next = self
                        .files
                        .directory
                        .open_with(temporary, &writable)?
                        .into_std();
                    private::check_handle(&next)?;
                    if !crate::same_file(&file, &next)? {
                        return Err(Error::Private);
                    }
                    next.seek(SeekFrom::Start(old.len() as u64))?;
                    next.write_all(&bytes[old.len()..])?;
                    next.sync_all()?;
                }
            }
            Err(e) => return Err(e.into()),
        }
        self.files.check()?;
        match private::publish_new(&self.files.directory, temporary, REFUSAL) {
            Ok(()) => {}
            Err(private::Error::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        private::sync_directory(&self.files.directory)?;
        if self.files.refusal()?.as_ref() != Some(&expected) {
            return Err(Error::Quarantined);
        }
        self.files.check()?;
        // Atomic no-replace publication retains one private file. This API
        // has no final marker overwrite, removal or rollback operation.
        Ok(())
    }
    pub fn acquire() -> Result<Self, Error> {
        #[cfg(not(feature = "test-support"))]
        if !adoption_runtime_qualified_platform() {
            return Err(Error::Unavailable);
        }
        Self::at(&root()?)
    }
    fn at(root: &Path) -> Result<Self, Error> {
        let files = Files::open(root)?;
        files.lock.try_lock().map_err(|_| Error::Locked)?;
        files.check()?;
        let refusal_registry = files.refusal()?.map(|r| r.registry_id);
        Ok(Self {
            files,
            refusal_registry,
        })
    }
    pub fn check(&self) -> Result<(), Error> {
        self.files.check()
    }
    pub fn refusal_registry(&self) -> Option<&str> {
        self.refusal_registry.as_deref()
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn isolated_for_tests(root: &Path) -> Result<Self, Error> {
        Self::at(root)
    }
}

/// Explicit process-fixed synthetic root, available only in test builds that
/// opt into the test-support feature. No environment variable selects it.
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub mod test_support {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    struct Selection {
        root: Option<PathBuf>,
        attempted: bool,
    }
    static SELECTED: Mutex<Selection> = Mutex::new(Selection {
        root: None,
        attempted: false,
    });
    pub fn initialize(root: &Path) -> Result<(), Error> {
        let root = root.canonicalize()?;
        let directory = Dir::open_ambient_dir(&root, cap_std::ambient_authority())?;
        private::check_handle(&directory.into_std_file())?;
        let mut selected = SELECTED.lock().map_err(|_| Error::TestRoot)?;
        if selected.root.as_ref() == Some(&root) {
            return Ok(());
        }
        if selected.attempted || selected.root.is_some() {
            return Err(Error::TestRoot);
        }
        selected.root = Some(root);
        Ok(())
    }
    /// Return only the explicit process-fixed test root; never create/fallback.
    pub fn configured_root() -> Result<PathBuf, Error> { selected_root() }
    pub(super) fn selected_root() -> Result<PathBuf, Error> {
        let mut selected = SELECTED.lock().map_err(|_| Error::TestRoot)?;
        selected.attempted = true;
        selected.root.clone().ok_or(Error::TestRoot)
    }
    /// Explicit fixture helper. The private temporary directory lives for the
    /// test process, including all worker threads and concurrently running tests.
    pub fn initialize_process_root() -> Result<PathBuf, Error> {
        static TEMP: OnceLock<tempfile::TempDir> = OnceLock::new();
        if TEMP.get().is_none() {
            let created = tempfile::tempdir()?;
            let _ = TEMP.set(created);
        }
        let path = TEMP.get().ok_or(Error::TestRoot)?.path().join("authority");
        private::directory(&path)?;
        initialize(&path)?;
        Ok(path)
    }
    /// Create a fixture only after explicitly selecting the synthetic writer
    /// authority. This cannot fall back to an OS-owner directory.
    pub fn initialized_tempdir() -> std::io::Result<tempfile::TempDir> {
        initialize_process_root().map_err(std::io::Error::other)?;
        tempfile::tempdir()
    }
}

#[cfg(test)]
mod tests;
