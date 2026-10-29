//! Retained private scratch objects. Cleanup never traverses ambient symlinks.
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt, OpenOptionsMaybeDirExt};
use cap_std::fs::{Dir, OpenOptions};
use std::{
    ffi::OsString,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

fn storage_error(error: hagency_store::Error) -> io::Error {
    io::Error::other(error)
}
fn open_directory(parent: &Dir, name: &Path) -> io::Result<Dir> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .maybe_dir(true);
    let file = parent.open_with(name, &options)?.into_std();
    if !file.metadata()?.is_dir() {
        return Err(io::Error::other("directory required"));
    }
    Ok(Dir::from_std_file(file))
}
fn open_file(parent: &Dir, name: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = parent.open_with(name, &options)?.into_std();
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("regular file required"));
    }
    Ok(file)
}
fn private_child(parent: &Dir, path: &Path, name: &str) -> io::Result<Dir> {
    hagency_store::private::create_directory_new(&path.join(name)).map_err(storage_error)?;
    // Open via the retained parent, not an ambient alias after creation.
    let dir = open_directory(parent, Path::new(name))?;
    hagency_store::private::check_handle(&dir.try_clone()?.into_std_file())
        .map_err(storage_error)?;
    Ok(dir)
}
fn create_file(parent: &Dir, name: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
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
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt;
        use windows::Win32::{
            Foundation::{GENERIC_READ, GENERIC_WRITE},
            Storage::FileSystem::{WRITE_DAC, WRITE_OWNER},
        };
        options.access_mode(GENERIC_READ.0 | GENERIC_WRITE.0 | WRITE_DAC.0 | WRITE_OWNER.0);
    }
    let file = parent.open_with(name, &options)?.into_std();
    // This handle came only from successful create_new. Seal before any bytes.
    hagency_store::private::seal_created_file_handle(&file).map_err(storage_error)?;
    Ok(file)
}

#[derive(Debug)]
pub(crate) struct StageError {
    pub cleanup_unknown: bool,
    source: io::Error,
}
impl From<io::Error> for StageError {
    fn from(source: io::Error) -> Self {
        Self {
            cleanup_unknown: false,
            source,
        }
    }
}
impl std::fmt::Display for StageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} (cleanup unknown: {})",
            self.source, self.cleanup_unknown
        )
    }
}
impl std::error::Error for StageError {}

pub(crate) struct RunDirectory {
    path: PathBuf,
    directory: Dir,
    _lease: File,
}
impl RunDirectory {
    pub fn open(cache: &Path, ttl: Duration) -> io::Result<Self> {
        std::fs::create_dir_all(cache)?;
        let root_path = cache.join("private-shares-v1");
        hagency_store::private::directory(&root_path).map_err(storage_error)?;
        let root = Dir::open_ambient_dir(&root_path, cap_std::ambient_authority())?;
        hagency_store::private::check_handle(&root.try_clone()?.into_std_file())
            .map_err(storage_error)?;
        sweep_runs(&root, ttl)?;
        let name = format!("run_{}", uuid::Uuid::new_v4());
        let directory = private_child(&root, &root_path, &name)?;
        let lease = create_file(&directory, Path::new("lease"))?;
        lease.try_lock()?;
        Ok(Self {
            path: root_path.join(name),
            directory,
            _lease: lease,
        })
    }
    pub fn stage(
        &self,
        filename: &str,
        bytes: &[u8],
        ttl: Duration,
    ) -> Result<StagedFile, StageError> {
        let name = format!("share_{}", uuid::Uuid::new_v4());
        let directory = private_child(&self.directory, &self.path, &name)?;
        let mut filename = sanitize_filename::sanitize(filename);
        if filename.is_empty() {
            filename = "shared_file".into();
        }
        let filename = OsString::from(filename);
        let file_path = self.path.join(&name).join(&filename);
        let staged: io::Result<StagedFile> = (|| {
            let mut file = create_file(&directory, Path::new(&filename))?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            let file = open_file(&directory, Path::new(&filename))?;
            hagency_store::private::check_handle(&file).map_err(storage_error)?;
            Ok(StagedFile {
                path: file_path,
                directory: directory.try_clone()?,
                filename,
                identity: Some(same_file::Handle::from_file(file)?),
                expires: Instant::now() + ttl,
                bytes: bytes.len(),
            })
        })();
        match staged {
            Ok(file) => Ok(file),
            Err(original) => {
                // Keep a failed cleanup observable; never silently claim that
                // partially written plaintext was removed.
                if let Err(cleanup) =
                    remove_contents(&directory, None, 0).and_then(|()| directory.remove_open_dir())
                {
                    return Err(StageError {
                        cleanup_unknown: true,
                        source: io::Error::other(format!(
                            "private share preparation failed ({original}); cleanup unconfirmed ({cleanup})"
                        )),
                    });
                }
                Err(StageError::from(original))
            }
        }
    }
    pub fn remove(self) -> io::Result<()> {
        // Do not recursively bypass a failed original-file cleanup. Leave any
        // unresolved child for logged retry/crash cleanup instead.
        let Self {
            directory, _lease, ..
        } = self;
        drop(_lease);
        directory.remove_file("lease")?;
        directory.remove_open_dir()
    }
}

pub(crate) struct StagedFile {
    pub path: PathBuf,
    directory: Dir,
    filename: OsString,
    identity: Option<same_file::Handle>,
    pub expires: Instant,
    pub bytes: usize,
}
impl StagedFile {
    pub fn verify(&self) -> io::Result<()> {
        let current =
            same_file::Handle::from_file(open_file(&self.directory, Path::new(&self.filename))?)?;
        if self.identity.as_ref() != Some(&current) {
            return Err(io::Error::other("share file identity changed"));
        }
        // Native APIs need a path. Verify that this alias still names our file.
        let alias = same_file::Handle::from_file(
            hagency_store::private::open(&self.path, false).map_err(storage_error)?,
        )?;
        if self.identity.as_ref() != Some(&alias) {
            return Err(io::Error::other("share file path changed"));
        }
        Ok(())
    }
    pub fn remove(&mut self) -> io::Result<()> {
        self.remove_with(|directory, filename| directory.remove_file(filename))
    }
    pub(super) fn remove_with(
        &mut self,
        unlink: impl FnOnce(&Dir, &std::ffi::OsStr) -> io::Result<()>,
    ) -> io::Result<()> {
        let current = match open_file(&self.directory, Path::new(&self.filename)) {
            Ok(file) => same_file::Handle::from_file(file)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.identity.take();
                return self.directory.try_clone()?.remove_open_dir();
            }
            Err(error) => return Err(error),
        };
        if self.identity.as_ref() != Some(&current) {
            return Err(io::Error::other("refused replaced share file cleanup"));
        }
        drop(current);
        // Retained read handles permit delete sharing on Windows. Keep the ID
        // through unlink: a transient sharing/IO failure must remain retryable.
        unlink(&self.directory, &self.filename)?;
        self.identity.take();
        self.directory.try_clone()?.remove_open_dir()
    }
}

fn sweep_runs(root: &Dir, ttl: Duration) -> io::Result<()> {
    for entry in root.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("run_") {
            continue;
        }
        let Ok(directory) = open_directory(root, Path::new(&name)) else {
            continue;
        };
        let lease = match open_file(&directory, Path::new("lease")) {
            Ok(file) => {
                hagency_store::private::check_handle(&file).map_err(storage_error)?;
                if file.try_lock().is_err() {
                    continue;
                }
                Some(file)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let age = directory
                    .dir_metadata()?
                    .modified()?
                    .into_std()
                    .elapsed()
                    .unwrap_or_default();
                if age < ttl {
                    continue;
                }
                None
            }
            Err(error) => return Err(error),
        };
        // Missing/expired processes only. Keep the original lock through traversal.
        remove_contents(&directory, None, 0)?;
        drop(lease);
        directory.remove_open_dir()?;
    }
    Ok(())
}

/// Legacy plaintext scratch cleanup is confined by retained directories and
/// NoFollow opens. No symlink or Windows reparse subtree is traversed.
pub(crate) fn sweep_legacy(cache: &Path, ttl: Duration) -> io::Result<()> {
    let base = Dir::open_ambient_dir(cache, cap_std::ambient_authority())?;
    let directory = match open_directory(&base, Path::new("temp")) {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    remove_contents(&directory, SystemTime::now().checked_sub(ttl), 0)
}
fn remove_contents(directory: &Dir, cutoff: Option<SystemTime>, depth: usize) -> io::Result<()> {
    if depth > 8 {
        return Err(io::Error::other("scratch directory nesting limit"));
    }
    for entry in directory.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        // Metadata is explicitly nofollow; relative open refuses links again.
        let metadata = directory.symlink_metadata(&name)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            let child = open_directory(directory, Path::new(&name))?;
            remove_contents(&child, cutoff, depth + 1)?;
            // An active lease or unexpired legacy file makes this nonempty.
            let _ = child.remove_open_dir();
        } else if metadata.is_file()
            && cutoff.is_none_or(|cutoff| metadata.modified().is_ok_and(|m| m.into_std() < cutoff))
        {
            let original = same_file::Handle::from_file(open_file(directory, Path::new(&name))?)?;
            let current = same_file::Handle::from_file(open_file(directory, Path::new(&name))?)?;
            if original != current {
                return Err(io::Error::other("scratch entry changed"));
            }
            drop((original, current));
            directory.remove_file(name)?;
        }
    }
    Ok(())
}
