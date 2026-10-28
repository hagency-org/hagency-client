//! Physical, bounded adoption custody. All paths come from retained owner scope.
use super::*;
use hagency_store::management::{ArchiveJournalBinding, PrivateManagementJournal};
use hagency_store::owner_resources::adoption::{
    AdoptionInventory, AdoptionTransfer, VerifiedAdoptionArchives,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Seek, SeekFrom, Write};
const MAX_FILE: u64 = 32 * 1024 * 1024;
mod empty_device;

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BackupEntry {
    name: String,
    bytes: u64,
    sha256: String,
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Backup {
    version: u16,
    migration_id: String,
    registry_id: String,
    source_binding: String,
    inventory_digest: String,
    files: Vec<BackupEntry>,
}
fn storage(_: impl std::fmt::Display) -> &'static str {
    "source_adoption_storage_unavailable"
}
fn digest(value: &impl Serialize) -> Result<String, &'static str> {
    hagency_core::canonical::digest(&serde_json::to_value(value).map_err(storage)?).map_err(storage)
}
fn regular_identity(file: &File) -> Result<serde_json::Value, &'static str> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = file.metadata().map_err(storage)?;
        Ok(serde_json::json!({"device":m.dev().to_string(),"inode":m.ino().to_string()}))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Err("source_adoption_platform_unqualified")
    }
}
impl Sources {
    /// A display-only snapshot while the original writer is still live. No
    /// source lock is taken or writer retired. Confirmation must recapture this
    /// inventory under exclusive custody before using it for initialization.
    pub(in crate::console::native::source_quiescence) fn empty_device_inventory(
        &self, profile: &str, owner: &str,
    ) -> Result<(String, hagency_store::owner_resources::adoption::EmptyDeviceInventory), &'static str> {
        self.check()?;
        let binding = self.empty_device_binding(profile)?;
        let mut domain = open_db(&self.domain, false)?;
        let mut personal = self.personal.as_ref().map(|p| open_db(p, false)).transpose()?;
        let inventory = hagency_store::owner_resources::adoption::EmptyDeviceInventory::inspect(
            &mut domain, personal.as_mut(), profile, owner,
        ).map_err(|_| "empty_device_history_not_empty")?;
        self.check()?;
        if self.empty_device_binding(profile)? != binding {
            return Err("source_files_changed");
        }
        Ok((binding, inventory))
    }
    /// A separate binding version can describe a genuinely absent personal
    /// source. Existing v1 populated migrations keep their original binding.
    pub(in crate::console::native::source_quiescence) fn empty_device_binding(&self, profile: &str) -> Result<String, &'static str> {
        self.check()?;
        let directory_id = |d: &Directory| {
            hagency_platform::directory_identity(
                &d.handle.try_clone().map_err(storage)?.into_std_file(),
            ).map_err(storage)
        };
        let personal = match &self.personal {
            Some(p) => serde_json::json!({"kind":"present","directory":directory_id(&p.directory)?,"file":regular_identity(&p.main)?}),
            None => serde_json::json!({"kind":"absent","parent":directory_id(&self.personal_parent)?,"name":self.personal_name,"emptyDirectory":self.personal_empty.as_ref().map(directory_id).transpose()?}),
        };
        digest(&serde_json::json!({"version":2,"kind":"empty_device","profile":profile,
            "domainDirectory":directory_id(&self.domain.directory)?,
            "domain":regular_identity(&self.domain.main)?,"domainLock":regular_identity(&self.domain_lock)?,
            "personal":personal}))
    }
    /// Read-only preview for an existing original88 pre-commit record. Requires
    /// exact retained physical identities and complete inventory digest. Normal
    /// archive openers and source retirement/fencing are never called here.
    pub(in crate::console::native::source_quiescence) fn saved_task_preview(
        &self,
        profile: &str,
        owner: &str,
        binding: &str,
        inventory_digest: &str,
        alerts: bool,
    ) -> Result<crate::console::native::adoption_handoff::TaskPrepareReview, &'static str> {
        if self.binding(profile)? != binding {
            return Err("source_adoption_binding_changed");
        }
        self.domain_lock
            .try_lock()
            .map_err(|_| "source_domain_busy")?;
        let result = (|| {
            let mut domain = open_db(&self.domain, false)?;
            let mut personal = open_db(
                self.personal
                    .as_ref()
                    .ok_or("personal_history_not_present")?,
                false,
            )?;
            // inspect_with_operator_tasks accepts only original unfenced app IDs.
            let inventory = if alerts {
                AdoptionInventory::inspect_with_operator_history(
                    &mut domain,
                    &mut personal,
                    profile,
                    owner,
                )
            } else {
                AdoptionInventory::inspect_with_operator_tasks(
                    &mut domain,
                    &mut personal,
                    profile,
                    owner,
                )
            }
            .map_err(|_| "source_adoption_preview_unavailable")?;
            if !inventory
                .matches_inventory_digest(inventory_digest)
                .map_err(storage)?
            {
                return Err("source_adoption_review_changed");
            }
            let preview = inventory.operator_task_preview().map_err(storage)?;
            let value = crate::console::native::adoption_handoff::TaskPrepareReview {
                resources: inventory.reviewed_resources().map_err(storage)?,
                tasks: preview.task_count(),
                comments: preview.comment_count(),
                alerts: preview.alert_count(),
                alert_notes: preview.alert_note_count(),
            };
            self.check()?;
            Ok(value)
        })();
        let unlock = self.domain_lock.unlock().map_err(|_| "source_domain_busy");
        let value = result?;
        unlock?;
        Ok(value)
    }

    pub(in crate::console::native::source_quiescence) fn open_adopted_journal(
        &self,
        transfer: &AdoptionTransfer,
        profile: &str,
    ) -> Result<PrivateManagementJournal, &'static str> {
        let binding = self.binding(profile)?;
        if binding != transfer.source_binding() {
            return Err("source_adoption_binding_changed");
        }
        let mut domain = open_db(&self.domain, false)?;
        let mut personal = open_db(
            self.personal
                .as_ref()
                .ok_or("personal_history_not_present")?,
            false,
        )?;
        transfer
            .inspect_active_archives(&mut domain, &mut personal, &binding)
            .map_err(storage)?;
        drop(domain);
        drop(personal);
        self.check()?;
        let journal = PrivateManagementJournal::open_existing(
            &self.domain.directory.path,
            transfer.archive_binding().map_err(storage)?,
        )
        .map_err(storage)?;
        self.check()?;
        Ok(journal)
    }
    /// During our own rollback-journal transaction, the journal may exist. The
    /// immutable namespace identities are still checked before each commit.
    pub(in crate::console::native::source_quiescence) fn adoption_identity_check(
        &self,
    ) -> Result<(), &'static str> {
        self.personal.as_ref().ok_or("personal_history_not_present")?;
        self.empty_device_identity_check()
    }
    pub(in crate::console::native::source_quiescence) fn empty_device_identity_check(
        &self,
    ) -> Result<(), &'static str> {
        for d in std::iter::once(&self.domain).chain(self.personal.iter()) {
            d.directory.check_file(d.name, &d.main)?;
            for (suffix, file) in [("-wal", &d.wal), ("-shm", &d.shm)] {
                let name = format!("{}{suffix}", d.name);
                if let Some(file) = file {
                    d.directory.check_file(&name, file)?;
                } else if d.directory.handle.symlink_metadata(&name).is_ok() {
                    return Err("source_files_changed");
                }
            }
        }
        self.domain
            .directory
            .check_file("domain.lock", &self.domain_lock)?;
        self.personal_parent.check()?;
        if self.personal.is_none() {
            let absent = if let Some(directory) = &self.personal_empty {
                directory.check()?;
                directory.handle.symlink_metadata("agent-local.sqlite")
            } else {
                self.personal_parent.handle.symlink_metadata(&self.personal_name)
            };
            if !matches!(absent,Err(e) if e.kind()==std::io::ErrorKind::NotFound) {
                return Err("source_files_changed");
            }
        }
        Ok(())
    }
    pub(in crate::console::native::source_quiescence) fn binding(
        &self,
        profile: &str,
    ) -> Result<String, &'static str> {
        self.check()?;
        let personal = self
            .personal
            .as_ref()
            .ok_or("personal_history_not_present")?;
        digest(&serde_json::json!({"version":1,"profile":profile,
            "domainDirectory":hagency_platform::directory_identity(&self.domain.directory.handle.try_clone().map_err(storage)?.into_std_file()).map_err(storage)?,
            "domain":regular_identity(&self.domain.main)?,"domainLock":regular_identity(&self.domain_lock)?,
            "personalDirectory":hagency_platform::directory_identity(&personal.directory.handle.try_clone().map_err(storage)?.into_std_file()).map_err(storage)?,
            "personal":regular_identity(&personal.main)?}))
    }
}

impl LockedSources {
    pub(in crate::console::native::source_quiescence) fn empty_device_inventory(
        &self, profile: &str, owner: &str,
    ) -> Result<(String, hagency_store::owner_resources::adoption::EmptyDeviceInventory), &'static str> {
        self.check()?;
        let result = self.sources.empty_device_inventory(profile, owner)?;
        self.check()?;
        Ok(result)
    }
}

fn open_db(files: &Database, write: bool) -> Result<Connection, &'static str> {
    files.check()?;
    let flags = if write {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let db = Connection::open_with_flags(
        files.directory.path.join(files.name),
        flags | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(storage)?;
    db.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
        .map_err(storage)?;
    db.busy_timeout(Duration::from_millis(100))
        .map_err(storage)?;
    files.check()?;
    Ok(db)
}
fn bounded_read(mut file: File, max: u64, gate: &Gate) -> Result<Vec<u8>, &'static str> {
    let length = file.metadata().map_err(storage)?.len();
    if length > max {
        return Err("source_adoption_backup_capacity");
    }
    let mut bytes = Vec::with_capacity(length as usize);
    let mut part = [0u8; 65536];
    loop {
        gate.check()?;
        let n = file.read(&mut part).map_err(storage)?;
        if n == 0 {
            break;
        }
        if bytes.len().saturating_add(n) > max as usize {
            return Err("source_adoption_backup_capacity");
        }
        bytes.extend_from_slice(&part[..n]);
    }
    Ok(bytes)
}
fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn save_new(
    directory: &Directory,
    name: &str,
    bytes: &[u8],
    gate: &Gate,
) -> Result<(), &'static str> {
    gate.check()?;
    match directory.file(name, false) {
        Ok(file) => {
            return if bounded_read(file, MAX_FILE, gate)? == bytes {
                Ok(())
            } else {
                Err("source_adoption_backup_changed")
            };
        }
        Err(_) if directory.handle.symlink_metadata(name).is_ok() => {
            return Err("source_adoption_backup_changed");
        }
        Err(_) => {}
    }
    let partial = format!("{name}.partial");
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
    let mut file = match directory.handle.open_with(&partial, &options) {
        Ok(f) => {
            let f = f.into_std();
            private::seal_created_file_handle(&f).map_err(storage)?;
            f
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            directory.file(&partial, true)?
        }
        Err(e) => return Err(storage(e)),
    };
    let previous = bounded_read(directory.file(&partial, false)?, MAX_FILE, gate)?;
    if !bytes.starts_with(&previous) {
        return Err("source_adoption_backup_changed");
    }
    file.seek(SeekFrom::Start(previous.len() as u64))
        .map_err(storage)?;
    for part in bytes[previous.len()..].chunks(65536) {
        gate.check()?;
        file.write_all(part).map_err(storage)?;
    }
    file.sync_all().map_err(storage)?;
    directory.check_file(&partial, &file)?;
    gate.check()?;
    hagency_platform::private_state::publish_new(&directory.handle, &partial, name)
        .map_err(storage)?;
    directory.check_file(name, &file)?;
    gate.check()
}
impl LockedSources {
    pub(in crate::console::native::source_quiescence) fn identities(&self) -> Arc<Sources> {
        self.sources.clone()
    }
    pub(in crate::console::native::source_quiescence) fn inventory(
        &self,
        profile: &str,
        owner: &str,
    ) -> Result<AdoptionInventory, &'static str> {
        self.inventory_mode(profile, owner, false, false)
    }
    pub(in crate::console::native::source_quiescence) fn inventory_with_operator_tasks(
        &self,
        profile: &str,
        owner: &str,
    ) -> Result<AdoptionInventory, &'static str> {
        self.inventory_mode(profile, owner, true, false)
    }
    pub(in crate::console::native::source_quiescence) fn inventory_with_operator_history(
        &self,
        profile: &str,
        owner: &str,
    ) -> Result<AdoptionInventory, &'static str> {
        self.inventory_mode(profile, owner, true, true)
    }
    fn inventory_mode(
        &self,
        profile: &str,
        owner: &str,
        tasks: bool,
        alerts: bool,
    ) -> Result<AdoptionInventory, &'static str> {
        self.check()?;
        let mut d = open_db(&self.sources.domain, false)?;
        let mut p = open_db(
            self.sources
                .personal
                .as_ref()
                .ok_or("personal_history_not_present")?,
            false,
        )?;
        let result = if alerts {
            AdoptionInventory::inspect_with_operator_history(&mut d, &mut p, profile, owner)
        } else if tasks {
            AdoptionInventory::inspect_with_operator_tasks(&mut d, &mut p, profile, owner)
        } else {
            AdoptionInventory::inspect(&mut d, &mut p, profile, owner)
        }
        .map_err(storage)?;
        self.check()?;
        Ok(result)
    }
    fn backup(
        &self,
        transfer: &AdoptionTransfer,
        registry: &str,
        binding: &str,
        gate: &Gate,
    ) -> Result<(), &'static str> {
        self.check()?;
        let personal = self
            .sources
            .personal
            .as_ref()
            .ok_or("personal_history_not_present")?;
        let path = self
            .sources
            .domain
            .directory
            .path
            .join(format!("adoption_{}", transfer.migration_id()));
        private::directory(&path).map_err(storage)?;
        let dir = Directory::open(&path)?;
        if dir.handle.symlink_metadata("backup.json").is_ok() {
            let backup: Backup = serde_json::from_slice(&bounded_read(
                dir.file("backup.json", false)?,
                16384,
                gate,
            )?)
            .map_err(storage)?;
            if backup.version != 1
                || backup.migration_id != transfer.migration_id()
                || backup.registry_id != registry
                || backup.source_binding != binding
                || backup.inventory_digest != transfer.inventory_digest()
                || !(2..=4).contains(&backup.files.len())
            {
                return Err("source_adoption_backup_changed");
            }
            let expected = source_files(&self.sources.domain, personal);
            if backup
                .files
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>()
                != expected.iter().map(|e| e.0.as_str()).collect::<Vec<_>>()
            {
                return Err("source_adoption_backup_changed");
            }
            for entry in backup.files {
                let bytes = bounded_read(dir.file(&entry.name, false)?, MAX_FILE, gate)?;
                if bytes.len() as u64 != entry.bytes || hash_bytes(&bytes) != entry.sha256 {
                    return Err("source_adoption_backup_changed");
                }
            }
        } else {
            // No manifest means source fencing has not started. Never replace
            // the original backup with bytes from an already-fenced database.
            for database in [&self.sources.domain, personal] {
                let db = open_db(database, false)?;
                if db
                    .pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0))
                    .map_err(storage)?
                    == 0x48415231
                {
                    return Err("source_adoption_backup_missing");
                }
            }
            let mut entries = Vec::new();
            for (name, database, original) in source_files(&self.sources.domain, personal) {
                let bytes =
                    bounded_read(database.directory.file(&original, false)?, MAX_FILE, gate)?;
                save_new(&dir, &name, &bytes, gate)?;
                entries.push(BackupEntry {
                    name,
                    bytes: bytes.len() as u64,
                    sha256: hash_bytes(&bytes),
                });
            }
            let backup = Backup {
                version: 1,
                migration_id: transfer.migration_id().into(),
                registry_id: registry.into(),
                source_binding: binding.into(),
                inventory_digest: transfer.inventory_digest().into(),
                files: entries,
            };
            save_new(
                &dir,
                "backup.json",
                &serde_json::to_vec(&backup).map_err(storage)?,
                gate,
            )?;
        }
        dir.check()?;
        self.check()?;
        gate.check()
    }
    pub(in crate::console::native::source_quiescence) fn fence(
        &mut self,
        transfer: &AdoptionTransfer,
        registry: &str,
        binding: &str,
        gate: &Gate,
        lease: &AdoptionExclusiveLease,
    ) -> Result<VerifiedAdoptionArchives, &'static str> {
        self.check()?;
        if let Some(db) = &self.personal
            && db.is_autocommit()
        {
            db.execute_batch("BEGIN IMMEDIATE").map_err(storage)?;
        }
        let mut d = open_db(&self.sources.domain, true)?;
        let mut p = open_db(
            self.sources
                .personal
                .as_ref()
                .ok_or("personal_history_not_present")?,
            true,
        )?;
        transfer
            .inspect_source_state(&mut d, &mut p, binding)
            .map_err(storage)?;
        self.backup(transfer, registry, binding, gate)?;
        lease
            .publish_refusal(transfer.migration_id(), transfer.inventory_digest())
            .map_err(storage)?;
        gate.check()?;
        transfer
            .fence_locked_personal(
                self.personal
                    .as_ref()
                    .ok_or("personal_history_not_present")?,
            )
            .map_err(storage)?;
        transfer
            .fence_sources(&mut d, &mut p, binding)
            .map_err(storage)?;
        drop(d);
        drop(p);
        self.check()?;
        let mut d = open_db(&self.sources.domain, false)?;
        let mut p = open_db(
            self.sources
                .personal
                .as_ref()
                .ok_or("personal_history_not_present")?,
            false,
        )?;
        let proof = transfer.inspect_archives(&mut d, &mut p).map_err(storage)?;
        self.check()?;
        gate.check()?;
        Ok(proof)
    }
    pub(in crate::console::native::source_quiescence) fn reacquire(
        &self,
    ) -> Result<(), &'static str> {
        self.check()?;
        self.sources
            .domain_lock
            .try_lock()
            .map_err(|_| "source_domain_busy")
    }
    pub(in crate::console::native::source_quiescence) fn journal(
        &self,
        binding: ArchiveJournalBinding,
    ) -> Result<PrivateManagementJournal, &'static str> {
        self.check()?;
        self.sources.domain_lock.unlock().map_err(storage)?;
        match PrivateManagementJournal::open_existing(&self.sources.domain.directory.path, binding)
        {
            Ok(journal) => Ok(journal),
            Err(error) => {
                let _ = self.sources.domain_lock.try_lock();
                Err(storage(error))
            }
        }
    }
}
fn source_files<'a>(
    domain: &'a Database,
    personal: &'a Database,
) -> Vec<(String, &'a Database, String)> {
    let mut result = Vec::new();
    for (prefix, d) in [("domain", domain), ("personal", personal)] {
        result.push((format!("{prefix}.sqlite"), d, d.name.into()));
        if d.wal.is_some() {
            result.push((format!("{prefix}.sqlite-wal"), d, format!("{}-wal", d.name)));
        }
    }
    result
}
