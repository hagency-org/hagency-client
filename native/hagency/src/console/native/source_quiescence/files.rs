//! Existing-file locks only. No normal opener, recovery, schema or marker write.
use super::*;
#[cfg(feature = "resource-adoption")]
pub(super) mod adoption;
#[cfg(feature = "resource-adoption")]
pub(super) mod graph_history;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::fs::{Dir, OpenOptions};
use hagency_store::private;
use rusqlite::{Connection, OpenFlags, config::DbConfig};
use std::{
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
};

struct Directory {
    path: PathBuf,
    handle: Dir,
}
impl Directory {
    fn open(path: &Path) -> Result<Self, &'static str> {
        let mut anchor = PathBuf::new();
        let mut names = Vec::new();
        for part in path.components() {
            match part {
                Component::RootDir | Component::Prefix(_) => anchor.push(part.as_os_str()),
                Component::Normal(name) => names.push(name),
                _ => return Err("source_files_changed"),
            }
        }
        if !path.is_absolute() {
            return Err("source_files_changed");
        }
        let mut handle = Dir::open_ambient_dir(anchor, cap_std::ambient_authority())
            .map_err(|_| "source_files_unavailable")?;
        for name in names {
            handle = handle
                .open_dir_nofollow(name)
                .map_err(|_| "source_files_unavailable")?;
        }
        private::check_handle(
            &handle
                .try_clone()
                .map_err(|_| "source_files_unavailable")?
                .into_std_file(),
        )
        .map_err(|_| "source_files_unavailable")?;
        Ok(Self {
            path: path.into(),
            handle,
        })
    }
    fn file(&self, name: &str, write: bool) -> Result<File, &'static str> {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(write)
            .follow(FollowSymlinks::No)
            .nonblock(true);
        let file = self
            .handle
            .open_with(name, &options)
            .map_err(|_| "source_files_unavailable")?
            .into_std();
        private::check_handle(&file).map_err(|_| "source_files_unavailable")?;
        if !file
            .metadata()
            .map_err(|_| "source_files_unavailable")?
            .is_file()
        {
            return Err("source_files_unavailable");
        }
        Ok(file)
    }
    fn check(&self) -> Result<(), &'static str> {
        let current = Self::open(&self.path)?;
        if !hagency_platform::same_directory(
            &self
                .handle
                .try_clone()
                .map_err(|_| "source_files_changed")?
                .into_std_file(),
            &current.handle.into_std_file(),
        )
        .map_err(|_| "source_files_changed")?
        {
            return Err("source_files_changed");
        }
        Ok(())
    }
    fn check_file(&self, name: &str, file: &File) -> Result<(), &'static str> {
        self.check()?;
        if !hagency_platform::same_file(file, &self.file(name, false)?)
            .map_err(|_| "source_files_changed")?
        {
            return Err("source_files_changed");
        }
        Ok(())
    }
}
struct Database {
    directory: Directory,
    name: &'static str,
    main: File,
    wal: Option<File>,
    shm: Option<File>,
}
impl Database {
    fn open(directory: Directory, name: &'static str) -> Result<Self, &'static str> {
        let main = directory.file(name, false)?;
        let mut header = [0; 100];
        (&main)
            .read_exact(&mut header)
            .map_err(|_| "source_schema_unavailable")?;
        if &header[..16] != b"SQLite format 3\0" {
            return Err("source_schema_unavailable");
        }
        if directory
            .handle
            .symlink_metadata(format!("{name}-journal"))
            .is_ok()
        {
            return Err("source_recovery_required");
        }
        let (wal, shm) = if header[18] == 2 && header[19] == 2 {
            (
                Some(
                    directory
                        .file(&format!("{name}-wal"), false)
                        .map_err(|_| "source_recovery_required")?,
                ),
                Some(
                    directory
                        .file(&format!("{name}-shm"), false)
                        .map_err(|_| "source_recovery_required")?,
                ),
            )
        } else {
            (None, None)
        };
        Ok(Self {
            directory,
            name,
            main,
            wal,
            shm,
        })
    }
    fn check(&self) -> Result<(), &'static str> {
        self.directory.check_file(self.name, &self.main)?;
        for (suffix, file) in [("-wal", &self.wal), ("-shm", &self.shm)] {
            let name = format!("{}{suffix}", self.name);
            if let Some(file) = file {
                self.directory.check_file(&name, file)?;
            } else if self.directory.handle.symlink_metadata(name).is_ok() {
                return Err("source_files_changed");
            }
        }
        if self
            .directory
            .handle
            .symlink_metadata(format!("{}-journal", self.name))
            .is_ok()
        {
            return Err("source_recovery_required");
        }
        Ok(())
    }
}
pub(super) struct Sources {
    domain: Database,
    domain_lock: File,
    personal: Option<Database>,
    personal_empty: Option<Directory>,
    personal_parent: Directory,
    personal_name: String,
}
impl Sources {
    pub fn capture(root: &Path, profile: &str) -> Result<Self, &'static str> {
        let owners = Directory::open(&root.join("owned-agent-owners"))?;
        let domain_dir = Directory::open(&owners.path.join(format!("management_{profile}")))?;
        let domain_lock = domain_dir.file("domain.lock", true)?;
        if domain_lock
            .metadata()
            .map_err(|_| "source_files_unavailable")?
            .len()
            != 0
        {
            return Err("source_files_unavailable");
        }
        let domain = Database::open(domain_dir, "domain.sqlite3")?;
        let personal_name = format!("owner_{profile}");
        let mut personal_empty = None;
        let personal = match owners.handle.symlink_metadata(&personal_name) {
            Ok(_) => {
                let directory = Directory::open(&owners.path.join(&personal_name))?;
                match directory.handle.symlink_metadata("agent-local.sqlite") {
                    Ok(_) => Some(Database::open(directory, "agent-local.sqlite")?),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        personal_empty = Some(directory);
                        None
                    }
                    Err(_) => return Err("source_files_unavailable"),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err("source_files_unavailable"),
        };
        let value = Self {
            domain,
            domain_lock,
            personal,
            personal_empty,
            personal_parent: owners,
            personal_name,
        };
        value.check()?;
        Ok(value)
    }
    pub fn check(&self) -> Result<(), &'static str> {
        self.domain.check()?;
        self.domain
            .directory
            .check_file("domain.lock", &self.domain_lock)?;
        self.personal_parent.check()?;
        if let Some(personal) = &self.personal {
            personal.check()?;
        } else {
            let absent = if let Some(directory) = &self.personal_empty {
                directory.check()?;
                directory.handle.symlink_metadata("agent-local.sqlite")
            } else {
                self.personal_parent
                    .handle
                    .symlink_metadata(&self.personal_name)
            };
            if !matches!(absent,Err(error) if error.kind()==std::io::ErrorKind::NotFound) {
                return Err("source_files_changed");
            }
        }
        Ok(())
    }
    pub fn lock(self, gate: Arc<Gate>) -> Result<LockedSources, &'static str> {
        gate.check()?;
        self.check()?;
        self.domain_lock
            .try_lock()
            .map_err(|_| "source_domain_busy")?;
        let personal = if let Some(files) = &self.personal {
            let db = Connection::open_with_flags(
                files.directory.path.join(files.name),
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            )
            .map_err(|_| "source_personal_busy")?;
            db.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
                .map_err(|_| "source_personal_busy")?;
            db.busy_timeout(Duration::from_millis(100))
                .map_err(|_| "source_personal_busy")?;
            let progress = gate.clone();
            db.progress_handler(1000, Some(move || progress.check().is_err()))
                .map_err(|_| "source_personal_busy")?;
            db.execute_batch("BEGIN IMMEDIATE")
                .map_err(|_| "source_personal_busy")?;
            Some(db)
        } else {
            None
        };
        self.check()?;
        gate.check()?;
        Ok(LockedSources {
            sources: Arc::new(self),
            personal,
        })
    }
}
pub(super) struct LockedSources {
    sources: Arc<Sources>,
    personal: Option<Connection>,
}
impl LockedSources {
    pub fn check(&self) -> Result<(), &'static str> {
        self.sources.check()
    }
    pub fn personal_present(&self) -> bool {
        self.personal.is_some()
    }
}
impl Drop for LockedSources {
    fn drop(&mut self) {
        if let Some(db) = &self.personal {
            let _ = db.execute_batch("ROLLBACK");
        }
    }
}
