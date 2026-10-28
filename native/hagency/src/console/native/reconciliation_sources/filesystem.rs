use super::*;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::fs::{Dir, OpenOptions};
use hagency_agent_local::reconciliation::{PersonalAuditAuthority, audit_readonly_connection};
use hagency_platform::{same_directory, same_file};
use hagency_store::{DomainRepository, private};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use std::{collections::BTreeSet, fs::File, io::Read, path::Path};

type Status = SourceObservationStatus;
type Result<T> = std::result::Result<T, Status>;
const MAX_ENTRIES: usize = 160;
const MAX_PROFILES: usize = 64;
const MAX_METADATA: u64 = 1024 * 1024;
const MARKER: &str = "hagency-client-owned-v1.json";
const MARKER_BYTES: &[u8] = b"{\"format\":\"hagency-owned-agent-client\",\"version\":1}\n";
const LOCK: &str = ".owner-client.lock";

fn io(error: std::io::Error) -> Status {
    if error.kind() == std::io::ErrorKind::NotFound {
        Status::NotPresent
    } else {
        Status::Unreadable
    }
}
pub(super) struct Directory {
    path: PathBuf,
    dir: Dir,
}
impl Directory {
    pub(super) fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(Status::Unreadable);
        }
        let mut anchor = PathBuf::new();
        let mut names = Vec::new();
        for component in path.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => anchor.push(component.as_os_str()),
                Component::Normal(name) => names.push(name.to_owned()),
                _ => return Err(Status::Unreadable),
            }
        }
        let mut dir = Dir::open_ambient_dir(anchor, cap_std::ambient_authority()).map_err(io)?;
        for name in names {
            dir = dir.open_dir_nofollow(name).map_err(io)?;
        }
        private::check_handle(&dir.try_clone().map_err(io)?.into_std_file())
            .map_err(|_| Status::Unreadable)?;
        Ok(Self {
            path: path.to_owned(),
            dir,
        })
    }
    fn child(&self, name: &str) -> Result<Self> {
        let dir = self.dir.open_dir_nofollow(name).map_err(io)?;
        private::check_handle(&dir.try_clone().map_err(io)?.into_std_file())
            .map_err(|_| Status::Unreadable)?;
        Ok(Self {
            path: self.path.join(name),
            dir,
        })
    }
    pub(super) fn check(&self) -> Result<()> {
        let current = Self::open(&self.path).map_err(|_| Status::Changed)?;
        if !same_directory(
            &self.dir.try_clone().map_err(io)?.into_std_file(),
            &current.dir.into_std_file(),
        )
        .map_err(io)?
        {
            return Err(Status::Changed);
        }
        Ok(())
    }
    fn file(&self, name: &str) -> Result<File> {
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No).nonblock(true);
        let file = self.dir.open_with(name, &options).map_err(io)?.into_std();
        private::check_handle(&file).map_err(|_| Status::Unreadable)?;
        if !file.metadata().map_err(io)?.is_file() {
            return Err(Status::Unreadable);
        }
        Ok(file)
    }
    fn bytes(&self, name: &str, limit: u64) -> Result<Vec<u8>> {
        let file = self.file(name)?;
        if file.metadata().map_err(io)?.len() > limit {
            return Err(Status::ObservationLimit);
        }
        let mut bytes = Vec::new();
        (&file)
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        if bytes.len() as u64 > limit {
            return Err(Status::ObservationLimit);
        }
        self.check_file(name, &file)?;
        Ok(bytes)
    }
    fn check_file(&self, name: &str, file: &File) -> Result<()> {
        self.check()?;
        private::check_handle(file).map_err(|_| Status::Changed)?;
        let current = self.file(name).map_err(|_| Status::Changed)?;
        if !same_file(file, &current).map_err(io)? {
            return Err(Status::Changed);
        }
        Ok(())
    }
    fn entries(&self) -> Result<Vec<String>> {
        let mut entries = Vec::new();
        for entry in self.dir.entries().map_err(io)?.take(MAX_ENTRIES + 1) {
            let name = entry
                .map_err(io)?
                .file_name()
                .into_string()
                .map_err(|_| Status::Unsupported)?;
            if name.len() > 255 {
                return Err(Status::ObservationLimit);
            }
            entries.push(name);
        }
        if entries.len() > MAX_ENTRIES {
            return Err(Status::ObservationLimit);
        }
        Ok(entries)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profiles {
    version: u32,
    accounts: Vec<Profile>,
    active_profile_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    origin: String,
    client_id: String,
    installation_id: String,
    name: String,
    owner: Option<String>,
    subject: Option<String>,
}
fn profile_metadata(root: &Directory) -> Result<BTreeSet<String>> {
    let bytes = root.bytes("server-login-profiles.json", MAX_METADATA)?;
    let metadata: Profiles = serde_json::from_slice(&bytes).map_err(|_| Status::Unsupported)?;
    if metadata.version != 1 || metadata.accounts.len() > MAX_PROFILES {
        return Err(Status::Unsupported);
    }
    let mut ids = BTreeSet::new();
    for p in metadata.accounts {
        if p.client_id.is_empty()
            || p.client_id.len() > 128
            || !(32..=128).contains(&p.installation_id.len())
            || !p
                .installation_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            || p.name.trim().is_empty()
            || p.name.chars().count() > 128
        {
            return Err(Status::Unsupported);
        }
        let id = profile_identity(
            &p.origin,
            &format!("{}auth/", p.origin),
            p.subject.as_deref().ok_or(Status::Unsupported)?,
            p.owner.as_deref().ok_or(Status::Unsupported)?,
        )
        .map_err(|_| Status::Unsupported)?;
        if !ids.insert(id) {
            return Err(Status::Unsupported);
        }
    }
    if metadata
        .active_profile_id
        .is_some_and(|id| !ids.contains(&id))
    {
        return Err(Status::Unsupported);
    }
    Ok(ids)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PersonalMarker {
    version: u32,
    origin: String,
    issuer: String,
    subject: String,
    owner: String,
}
fn personal_marker(dir: &Directory, scope: &Scope) -> Result<()> {
    let marker: PersonalMarker =
        serde_json::from_slice(&dir.bytes("profile-identity.json", 16 * 1024)?)
            .map_err(|_| Status::Unsupported)?;
    if marker.version != 1
        || marker.origin != scope.device.origin()
        || (marker.issuer != scope.device.issuer()
            && marker.issuer
                != crate::server_admission::profile_namespace_issuer(scope.device.origin()))
        || marker.subject != scope.device.subject()
        || marker.owner != scope.device.owner_mxid()
    {
        return Err(Status::Unsupported);
    }
    Ok(())
}
pub(super) struct Database<'a> {
    directory: &'a Directory,
    name: &'static str,
    main: File,
    companions: Vec<(String, File)>,
}
impl<'a> Database<'a> {
    pub(super) fn existing(directory: &'a Directory, name: &'static str) -> Result<Self> {
        let main = directory.file(name)?;
        let mut header = [0u8; 100];
        (&main)
            .read_exact(&mut header)
            .map_err(|_| Status::Unsupported)?;
        if &header[..16] != b"SQLite format 3\0" {
            return Err(Status::Unsupported);
        }
        let journal = format!("{name}-journal");
        match directory.file(&journal) {
            Err(Status::NotPresent) => {}
            _ => return Err(Status::RecoveryRequired),
        }
        let mut companions = Vec::new();
        for suffix in ["-wal", "-shm"] {
            let path = format!("{name}{suffix}");
            match directory.file(&path) {
                Ok(file) => companions.push((path, file)),
                Err(Status::NotPresent) if header[18] != 2 && header[19] != 2 => {}
                Err(Status::NotPresent) => return Err(Status::RecoveryRequired),
                Err(error) => return Err(error),
            }
        }
        // An existing WAL requires an existing SHM. Never ask SQLite to create
        // one as a side effect of this observation, and never use immutable=1.
        if companions.len() == 1 {
            return Err(Status::RecoveryRequired);
        }
        let result = Self {
            directory,
            name,
            main,
            companions,
        };
        result.check()?;
        Ok(result)
    }
    pub(super) fn check(&self) -> Result<()> {
        self.directory.check_file(self.name, &self.main)?;
        for (name, file) in &self.companions {
            self.directory.check_file(name, file)?;
        }
        for suffix in ["-wal", "-shm", "-journal"] {
            let name = format!("{}{suffix}", self.name);
            if !self.companions.iter().any(|(old, _)| *old == name) {
                match self.directory.file(&name) {
                    Err(Status::NotPresent) => {}
                    _ => return Err(Status::Changed),
                }
            }
        }
        Ok(())
    }
    fn open(&self, scope: Arc<Scope>) -> Result<Connection> {
        self.check()?;
        // The pinned SQLite Unix/Windows VFS honors readonly_shm: it will
        // not create a missing shared-memory companion on a read-only open.
        // Use a library-encoded file URI so path characters cannot add options.
        let mut uri = reqwest::Url::from_file_path(self.directory.path.join(self.name))
            .map_err(|_| Status::Unreadable)?;
        uri.query_pairs_mut()
            .append_pair("mode", "ro")
            .append_pair("readonly_shm", "1");
        let connection = Connection::open_with_flags(
            uri.as_str(),
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NOFOLLOW
                | OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(sqlite_error)?;
        connection
            .busy_timeout(Duration::from_millis(100))
            .map_err(sqlite_error)?;
        connection
            .progress_handler(1000, Some(move || scope.check().is_err()))
            .map_err(sqlite_error)?;
        self.check()?;
        Ok(connection)
    }
}
fn sqlite_error(error: rusqlite::Error) -> Status {
    match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
            Status::Busy
        }
        Some(rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt) => {
            Status::Unsupported
        }
        _ => Status::Unreadable,
    }
}
fn domain(dir: &Directory, scope: Arc<Scope>) -> Result<ProfileLedgerAudit> {
    let files = Database::existing(dir, "domain.sqlite3")?;
    let mut db = files.open(scope.clone())?;
    let evidence =
        DomainRepository::audit_readonly_connection(&mut db, &scope.profile).map_err(|error| {
            match error {
                hagency_store::Error::Sqlite(error) => sqlite_error(error),
                hagency_store::Error::Capacity => Status::ObservationLimit,
                _ => Status::Unsupported,
            }
        })?;
    drop(db);
    files.check()?;
    Ok(evidence)
}
fn personal(dir: &Directory, scope: Arc<Scope>) -> Result<PersonalLedgerAudit> {
    personal_marker(dir, &scope)?;
    let files = Database::existing(dir, "agent-local.sqlite")?;
    let mut db = files.open(scope.clone())?;
    let gate = scope.clone();
    let authority = PersonalAuditAuthority::new(
        scope.device.owner_mxid().into(),
        scope.profile.clone(),
        scope.expires,
        move || gate.check().is_ok(),
    )
    .map_err(|_| Status::Unreadable)?;
    let evidence =
        audit_readonly_connection(&mut db, &authority).map_err(|_| Status::Unsupported)?;
    drop(db);
    files.check()?;
    Ok(evidence)
}
fn observation<T>(result: Result<T>) -> SourceObservation<T> {
    match result {
        Ok(evidence) => SourceObservation::inspected(evidence),
        Err(status) => SourceObservation::status(status),
    }
}
fn inspect_root(
    scope: Arc<Scope>,
    root: &Directory,
    selection: u32,
) -> Result<SourceRootObservation> {
    if root.bytes(MARKER, 128)? != MARKER_BYTES {
        return Err(Status::Unsupported);
    }
    let _lock = if selection > 0 {
        let lock = root.file(LOCK)?;
        lock.try_lock().map_err(|_| Status::Busy)?;
        Some(lock)
    } else {
        None
    };
    let metadata = profile_metadata(root);
    let mut result = SourceRootObservation::unavailable(selection, Status::NotPresent);
    result.status = Status::Inspected;
    result.profile_metadata = match &metadata {
        Ok(_) => Status::Inspected,
        Err(status) => *status,
    };
    let root_entries = root.entries()?;
    result.unrecognized_entries = root_entries
        .iter()
        .filter(|name| {
            !matches!(
                name.as_str(),
                MARKER
                    | LOCK
                    | "server-login.json"
                    | "server-login-sdk-resume.json"
                    | "server-login-revocations.json"
                    | "server-login-profiles.json"
                    | "owned-agent-owners"
                    | "owner-console.sock"
            )
        })
        .count() as u32;
    let owners = match root.child("owned-agent-owners") {
        Ok(owners) => owners,
        Err(Status::NotPresent) => {
            root.check()?;
            return Ok(result);
        }
        Err(status) => return Err(status),
    };
    let personal_name = format!("owner_{}", scope.profile);
    let management_name = format!("management_{}", scope.profile);
    for name in owners.entries()? {
        if name == personal_name || name == management_name {
            result.profile_directories_observed += 1;
        } else if let Some(id) = name
            .strip_prefix("owner_")
            .or_else(|| name.strip_prefix("management_"))
            && id.len() == 64
            && id
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            result.profile_directories_observed += 1;
            result.other_profile_directories += 1;
            if !metadata.as_ref().is_ok_and(|known| known.contains(id)) {
                result.unrecognized_entries += 1;
            }
        } else {
            result.unrecognized_entries += 1;
        }
    }
    result.domain = observation(
        owners
            .child(&management_name)
            .and_then(|dir| domain(&dir, scope.clone())),
    );
    result.personal = observation(
        owners
            .child(&personal_name)
            .and_then(|dir| personal(&dir, scope)),
    );
    owners.check()?;
    root.check()?;
    Ok(result)
}
pub(super) fn inspect(
    scope: Arc<Scope>,
    paths: Vec<PathBuf>,
) -> std::result::Result<SourceCensus, NativeError> {
    scope.check()?;
    let mut retained: Vec<Directory> = Vec::new();
    let mut roots = Vec::new();
    for (selection, path) in paths.iter().enumerate() {
        scope.check()?;
        let observation = match Directory::open(path) {
            Ok(root) => {
                let duplicate = retained.iter().any(|old| {
                    let same = || -> std::io::Result<bool> {
                        same_directory(
                            &old.dir.try_clone()?.into_std_file(),
                            &root.dir.try_clone()?.into_std_file(),
                        )
                    };
                    same().unwrap_or(true)
                });
                if duplicate {
                    SourceRootObservation::unavailable(selection as u32, Status::Unsupported)
                } else {
                    let result = inspect_root(scope.clone(), &root, selection as u32)
                        .unwrap_or_else(|status| {
                            SourceRootObservation::unavailable(selection as u32, status)
                        });
                    retained.push(root);
                    result
                }
            }
            Err(status) => SourceRootObservation::unavailable(selection as u32, status),
        };
        roots.push(observation);
        scope.check()?;
    }
    scope.check()?;
    Ok(SourceCensus {
        version: 1,
        scope: "current_authenticated_profile",
        adoption: "not_assessed",
        external_roots_complete: false,
        roots,
        blockers: vec![
            "external_roots_not_proven_complete",
            "provider_mapping_not_established",
            "migration_not_assessed",
            "old_writers_not_fenced",
        ],
    })
}
