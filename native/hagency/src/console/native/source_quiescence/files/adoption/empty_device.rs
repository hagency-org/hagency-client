//! Optional-source backup and fencing under the original physical custody.
use super::*;
use hagency_store::owner_resources::adoption::{EmptyDeviceTransfer, VerifiedEmptyDeviceArchives};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmptyBackup {
    version: u8,
    initialization_id: String,
    registry_id: String,
    source_binding: String,
    inventory_digest: String,
    personal_present: bool,
    files: Vec<BackupEntry>,
}
fn originals(s: &Sources) -> Vec<(String, &Database, String)> {
    let mut result = Vec::new();
    for (prefix, d) in
        std::iter::once(("domain", &s.domain)).chain(s.personal.iter().map(|p| ("personal", p)))
    {
        result.push((format!("{prefix}.sqlite"), d, d.name.into()));
        if d.wal.is_some() {
            result.push((format!("{prefix}.sqlite-wal"), d, format!("{}-wal", d.name)));
        }
    }
    result
}
impl Sources {
    pub(in crate::console::native::source_quiescence) fn open_empty_adopted_journal(
        &self,
        transfer: &EmptyDeviceTransfer,
        profile: &str,
    ) -> Result<PrivateManagementJournal, &'static str> {
        self.inspect_active_empty_transfer(transfer, profile)?;
        let journal = PrivateManagementJournal::open_existing(
            &self.domain.directory.path,
            transfer.archive_binding().map_err(storage)?,
        )
        .map_err(storage)?;
        self.check()?;
        Ok(journal)
    }
    pub(in crate::console::native::source_quiescence) fn inspect_empty_transfer(
        &self,
        transfer: &EmptyDeviceTransfer,
        profile: &str,
    ) -> Result<(), &'static str> {
        self.inspect_empty_transfer_mode(transfer, profile, false)
    }
    pub(in crate::console::native::source_quiescence) fn inspect_active_empty_transfer(
        &self,
        transfer: &EmptyDeviceTransfer,
        profile: &str,
    ) -> Result<(), &'static str> {
        self.inspect_empty_transfer_mode(transfer, profile, true)
    }
    fn inspect_empty_transfer_mode(
        &self,
        transfer: &EmptyDeviceTransfer,
        profile: &str,
        active: bool,
    ) -> Result<(), &'static str> {
        self.check()?;
        let binding = self.empty_device_binding(profile)?;
        if binding != transfer.source_binding() {
            return Err("source_adoption_binding_changed");
        }
        let mut d = open_db(&self.domain, false)?;
        let mut p = self
            .personal
            .as_ref()
            .map(|p| open_db(p, false))
            .transpose()?;
        if active {
            transfer.inspect_active_archives(&mut d, p.as_mut(), &binding)
        } else {
            transfer.inspect_source_state(&mut d, p.as_mut(), &binding)
        }
        .map_err(storage)?;
        self.check()
    }
}
impl LockedSources {
    fn empty_backup(
        &self,
        transfer: &EmptyDeviceTransfer,
        registry: &str,
        gate: &Gate,
    ) -> Result<(), &'static str> {
        self.check()?;
        let path = self
            .sources
            .domain
            .directory
            .path
            .join(format!("adoption_{}", transfer.initialization_id()));
        private::directory(&path).map_err(storage)?;
        let dir = Directory::open(&path)?;
        let original = originals(&self.sources);
        if dir.handle.symlink_metadata("backup.json").is_ok() {
            let saved: EmptyBackup = serde_json::from_slice(&bounded_read(
                dir.file("backup.json", false)?,
                16384,
                gate,
            )?)
            .map_err(storage)?;
            if saved.version != 2
                || saved.initialization_id != transfer.initialization_id()
                || saved.registry_id != registry
                || saved.source_binding != transfer.source_binding()
                || saved.inventory_digest != transfer.inventory_digest()
                || saved.personal_present != transfer.personal_present()
                || saved.files.len() != original.len()
                || saved
                    .files
                    .iter()
                    .map(|e| &e.name)
                    .ne(original.iter().map(|e| &e.0))
            {
                return Err("source_adoption_backup_changed");
            }
            for entry in saved.files {
                let bytes = bounded_read(dir.file(&entry.name, false)?, MAX_FILE, gate)?;
                if bytes.len() as u64 != entry.bytes || hash_bytes(&bytes) != entry.sha256 {
                    return Err("source_adoption_backup_changed");
                }
            }
        } else {
            for database in
                std::iter::once(&self.sources.domain).chain(self.sources.personal.iter())
            {
                let db = open_db(database, false)?;
                if db
                    .pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0))
                    .map_err(storage)?
                    == 0x48415231
                {
                    return Err("source_adoption_backup_missing");
                }
            }
            let mut files = Vec::new();
            for (name, d, original) in original {
                let bytes = bounded_read(d.directory.file(&original, false)?, MAX_FILE, gate)?;
                save_new(&dir, &name, &bytes, gate)?;
                files.push(BackupEntry {
                    name,
                    bytes: bytes.len() as u64,
                    sha256: hash_bytes(&bytes),
                });
            }
            let backup = EmptyBackup {
                version: 2,
                initialization_id: transfer.initialization_id().into(),
                registry_id: registry.into(),
                source_binding: transfer.source_binding().into(),
                inventory_digest: transfer.inventory_digest().into(),
                personal_present: transfer.personal_present(),
                files,
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
    pub(in crate::console::native::source_quiescence) fn fence_empty_device(
        &mut self,
        transfer: &EmptyDeviceTransfer,
        registry: &str,
        profile: &str,
        gate: &Gate,
        lease: &AdoptionExclusiveLease,
    ) -> Result<VerifiedEmptyDeviceArchives, &'static str> {
        self.check()?;
        if let Some(p) = &self.personal
            && p.is_autocommit()
        {
            p.execute_batch("BEGIN IMMEDIATE").map_err(storage)?;
        }
        self.sources.inspect_empty_transfer(transfer, profile)?;
        self.empty_backup(transfer, registry, gate)?;
        lease
            .publish_refusal(transfer.initialization_id(), transfer.inventory_digest())
            .map_err(storage)?;
        gate.check()?;
        if let Some(p) = &self.personal {
            transfer.fence_locked_personal(p).map_err(storage)?;
        }
        let mut d = open_db(&self.sources.domain, true)?;
        let mut p = self
            .sources
            .personal
            .as_ref()
            .map(|p| open_db(p, true))
            .transpose()?;
        transfer
            .fence_sources(&mut d, p.as_mut(), transfer.source_binding())
            .map_err(storage)?;
        drop(d);
        drop(p);
        self.check()?;
        let mut d = open_db(&self.sources.domain, false)?;
        let mut p = self
            .sources
            .personal
            .as_ref()
            .map(|p| open_db(p, false))
            .transpose()?;
        let proof = transfer
            .inspect_archives(&mut d, p.as_mut())
            .map_err(storage)?;
        self.check()?;
        gate.check()?;
        Ok(proof)
    }
}
