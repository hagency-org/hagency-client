//! Reuse original command custody only when every non-journal table is empty.
//! No source recovery, migration, second receipt store, or resource authority.
use super::*;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::fs::{Dir, OpenOptions};
use rusqlite::{OpenFlags, config::DbConfig};
use std::{
    fs::File,
    path::{Component, PathBuf},
};

fn server_operation(operation: IntentOperation) -> bool {
    use IntentOperation::*;
    matches!(
        operation,
        Decide
            | Revoke
            | ProjectRequest
            | ProjectDecide
            | ProjectRevoke
            | PoolAgentRequest
            | PoolAgentDecide
            | PoolAgentStop
            | PoolAgentRenew
            | PoolAgentReallocate
            | AgentRequest
            | AgentDecide
            | AgentCancel
            | AgentRevoke
            | AgentSettlementRequest
            | AgentTopUpRequest
            | AgentTopUpDecide
            | AgentTopUpCancel
            | AgentRunControl
            | AllocationDecide
            | AllocationRelease
    )
}

pub(super) fn check_command(command: &JournalCommand) -> Result<(), Error> {
    if command
        .intent_operation()
        .is_some_and(|op| !server_operation(op))
    {
        return Err(Error::ResourceSetupRequired);
    }
    Ok(())
}

fn validate_schema(source: &Connection, deadline: Instant) -> Result<(), Error> {
    #[cfg(any(test, feature = "resource-adoption"))]
    {
        crate::owner_resources::adoption::validate_journal_source_schema(
            source,
            crate::DOMAIN_SCHEMA_VERSION,
            deadline,
            false,
        )
    }
    #[cfg(not(any(test, feature = "resource-adoption")))]
    {
        let _ = (source, deadline);
        Err(Error::ResourceSetupRequired)
    }
}

pub(super) struct Files {
    path: PathBuf,
    dir: Dir,
    retained: Vec<(&'static str, File)>,
}
impl Files {
    fn directory(path: &Path) -> Result<Dir, Error> {
        if !path.is_absolute() {
            return Err(Error::Private);
        }
        let mut root = PathBuf::new();
        let mut names = Vec::new();
        for part in path.components() {
            match part {
                Component::RootDir | Component::Prefix(_) => root.push(part.as_os_str()),
                Component::Normal(name) => names.push(name.to_owned()),
                _ => return Err(Error::Private),
            }
        }
        let mut dir = Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
        for name in names {
            dir = dir.open_dir_nofollow(name)?;
        }
        crate::private::check_handle(&dir.try_clone()?.into_std_file())?;
        Ok(dir)
    }
    fn file(dir: &Dir, name: &str) -> Result<File, Error> {
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No).nonblock(true);
        let file = dir.open_with(name, &options)?.into_std();
        crate::private::check_handle(&file)?;
        if !file.metadata()?.is_file() {
            return Err(Error::Private);
        }
        Ok(file)
    }
    pub(super) fn check(&self) -> Result<(), Error> {
        let current = Self::directory(&self.path)?;
        if !hagency_platform::same_directory(
            &self.dir.try_clone()?.into_std_file(),
            &current.try_clone()?.into_std_file(),
        )? {
            return Err(Error::Private);
        }
        for (name, file) in &self.retained {
            crate::private::check_handle(file)?;
            if !hagency_platform::same_file(file, &Self::file(&current, name)?)? {
                return Err(Error::Private);
            }
        }
        // An archive or another journal requires an explicit custody handoff.
        for name in ["domain.sqlite3-journal", "server-management.sqlite3"] {
            match current.symlink_metadata(name) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Ok(_) => return Err(Error::ResourceSetupRequired),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

pub(super) fn open(
    directory: &Path,
    profile: &str,
) -> Result<(crate::database::Database, Files), Error> {
    // This is only a fallback for an explicitly refused installation. It
    // cannot be used as an alternate opener while legacy writers are admitted.
    match hagency_platform::legacy_writer::LegacyWriterGuard::acquire() {
        Err(hagency_platform::legacy_writer::Error::Refused) => {}
        _ => return Err(Error::ResourceSetupRequired),
    }
    let deadline = Instant::now() + DEADLINE;
    let dir = Files::directory(directory)?;
    let ownership = Files::file(&dir, "domain.lock")?;
    ownership.try_lock().map_err(|_| Error::Locked)?;
    let mut files = Files {
        path: directory.into(),
        dir,
        retained: vec![("domain.lock", ownership.try_clone()?)],
    };
    for name in ["domain.sqlite3", "domain.sqlite3-wal", "domain.sqlite3-shm"] {
        files.retained.push((name, Files::file(&files.dir, name)?));
    }
    files.check()?;
    let path = directory.join("domain.sqlite3");
    let mut source = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    source.busy_timeout(Duration::from_millis(100))?;
    source.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)?;
    source.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    source.progress_handler(1000, Some(move || Instant::now() >= deadline))?;
    validate_schema(&source, deadline)?;
    let audit = DomainRepository::audit_readonly_connection(&mut source, profile)?;
    let h = &audit.history;
    if audit.schema_version != crate::DOMAIN_SCHEMA_VERSION
        || audit.optional_history_tables_missing != 0 || !audit.counts_complete
        || !audit.resources_complete || h.resource_definitions.observed != 0
        || h.seat_definitions.observed != 0 || h.capacity_and_usage_rows.observed != 0
        || h.execution_rows.observed != 0 || h.other_history_rows.observed != 0
        // These receipts belong to local effects, not central HTTP commands.
        || h.command_receipts.observed != 0
    {
        return Err(Error::ResourceSetupRequired);
    }
    let oversized: bool = source.query_row(&format!(
        "SELECT EXISTS(SELECT 1 FROM management_intents WHERE length(actor_scope)<>64 OR length(command_id)>128 OR length(operation)>64 OR length(digest)<>64 OR length(CAST(input AS BLOB))>{} OR length(CAST(reply AS BLOB))>65536)",
        intents::INPUT_LIMIT_SQL), [], |r| r.get(0))?;
    if oversized {
        return Err(Error::Capacity);
    }
    let mut operations =
        source.prepare("SELECT DISTINCT operation FROM management_intents LIMIT 65")?;
    for operation in operations.query_map([], |r| r.get::<_, String>(0))? {
        let operation =
            serde_json::from_str(&operation?).map_err(|_| Error::ResourceSetupRequired)?;
        if !server_operation(operation) {
            return Err(Error::ResourceSetupRequired);
        }
    }
    drop(operations);
    let mode: String = source.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
    if mode != "wal" {
        return Err(Error::Schema);
    }
    drop(source);
    files.check()?;
    if Instant::now() >= deadline {
        return Err(Error::Busy);
    }
    // Preserve application_id, user_version, original rows and original lock.
    // CREATE is deliberately absent; the SQL authorizer is installed before
    // this connection is handed to the only worker which can write receipts.
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    connection.busy_timeout(Duration::from_millis(100))?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    files.check()?;
    Ok((
        crate::database::Database {
            connection,
            ownership,
        },
        files,
    ))
}
