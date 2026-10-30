//! Desktop owner state and in-process API facade. Only NativeOwner exposes an
//! OAuth callback listener; this type never starts an HTTP management server.
use crate::console::Console;
use salvo::prelude::*;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};
const MARKER: &str = "hagency-client-owned-v1.json";
const MARKER_BYTES: &[u8] = b"{\"format\":\"hagency-owned-agent-client\",\"version\":1}\n";
const LOCK: &str = ".owner-client.lock";
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("use a fresh private owner-client state directory; legacy or foreign data is refused")]
    State,
    #[error("another owner-client process is using this state directory")]
    AlreadyRunning,
    #[error("use a nonzero loopback listen address")]
    Address,
    #[error("native owner engine initialization failed")]
    Engine,
}

struct State {
    path: PathBuf,
    _lock: std::fs::File,
}
fn allowed_entry(name: &str) -> bool {
    matches!(name,MARKER|LOCK|"server-login.json"|"server-login-revocations.json"|"server-login-profiles.json"|"owned-agent-owners"|"owner-console.sock"|"credentials-v1"|".server-login-revocations.json.vault-lock"|".server-login-revocations.json.vault-journal")
        // Only this revoke document's private journal-owned candidate names are
        // admitted. Its Document decoder still verifies scope/hash/AEAD before
        // recovery; admission grants no session authority.
        || name.strip_prefix(".server-login-revocations.json.vault-").and_then(|n| n.strip_suffix(".tmp")).is_some_and(|id| id.len()==32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        // A crash while writing the journal itself can precede its rename.
        // Retain only this exact nonsecret temporary shape, without importing it.
        || name.strip_prefix(".server-login-revocations.json.vault-journal-").and_then(|n| n.strip_suffix(".tmp")).is_some_and(|id| id.len()==32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        // Interrupted private JSON replacement files are never opened/imported.
        || (name.len()==21 && name.starts_with('.') && name.ends_with(".tmp") && name.as_bytes()[1..17].iter().all(|b|b.is_ascii_hexdigit()))
}
fn prepare(path: &Path) -> Result<State, Error> {
    if !path.is_absolute() {
        return Err(Error::State);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| Error::State)?;
    }
    hagency_store::private::directory(path).map_err(|_| Error::State)?;
    let path = path.canonicalize().map_err(|_| Error::State)?;
    let existing: Vec<_> = std::fs::read_dir(&path)
        .map_err(|_| Error::State)?
        .collect::<Result<_, _>>()
        .map_err(|_| Error::State)?;
    let has_marker = path.join(MARKER).exists();
    // No old database/config bytes are read. Absence of our fresh format marker
    // permits only an empty directory, including after interrupted lock creation.
    if (!has_marker && existing.iter().any(|e| e.file_name() != LOCK))
        || existing
            .iter()
            .any(|e| !e.file_name().to_str().is_some_and(allowed_entry))
    {
        return Err(Error::State);
    }
    let lock = hagency_store::private::open(&path.join(LOCK), true)
        .or_else(|_| hagency_store::private::open(&path.join(LOCK), false))
        .map_err(|_| Error::State)?;
    lock.try_lock().map_err(|_| Error::AlreadyRunning)?;
    if has_marker {
        let bytes =
            hagency_store::private::read_secret(&path.join(MARKER)).map_err(|_| Error::State)?;
        if bytes != MARKER_BYTES {
            return Err(Error::State);
        }
    } else {
        hagency_store::private::replace(&path.join(MARKER), MARKER_BYTES)
            .map_err(|_| Error::State)?;
    }
    Ok(State { path, _lock: lock })
}
/// Initializes only the new format, and never imports old SQLite or credentials.
pub fn initialize(path: &Path) -> Result<PathBuf, Error> {
    Ok(prepare(path)?.path)
}
#[derive(Clone)]
pub struct OwnerHost {
    pub(crate) console: Console,
    pub(crate) authority: String,
    state: Arc<State>,
}
impl OwnerHost {
    /// Local in-process bootstrap capability; no network issuer is mounted.
    pub fn owner_access_ticket(&self) -> Result<String, Error> {
        self.console
            .owner_access_ticket()
            .map_err(|_| Error::Engine)
    }
    /// In-process native management. No web assets are loaded or presented.
    pub fn open_native(state: &Path, address: SocketAddr) -> Result<Self, Error> {
        Self::open_native_with_credentials(
            state,
            address,
            hagency_store::credential_vault::Vault::native(),
        )
    }
    pub(crate) fn open_native_with_credentials(
        state: &Path,
        address: SocketAddr,
        vault: hagency_store::credential_vault::Vault,
    ) -> Result<Self, Error> {
        if !address.ip().is_loopback() || address.port() == 0 {
            return Err(Error::Address);
        }
        let state = prepare(state)?;
        let console = Console::native_with_state_and_credentials(&state.path, vault)
            .map_err(|_| Error::Engine)?;
        Ok(Self {
            console,
            authority: address.to_string(),
            state: Arc::new(state),
        })
    }
    pub fn state_directory(&self) -> &Path {
        &self.state.path
    }
    pub fn router(self) -> Router {
        Router::new()
            .hoop(self)
            .push(Router::with_path("health").get(health))
            .push(Router::with_path("ready").get(health))
            .push(crate::console::owner_router())
    }
}

#[handler]
impl OwnerHost {
    async fn handle(&self, depot: &mut Depot) {
        depot.insert_typed(self.clone());
    }
}
#[handler]
async fn health(res: &mut Response) {
    res.render(Json(
        serde_json::json!({"ok":true,"mode":"owned-agent-client","fleet":false}),
    ));
}
#[cfg(test)]
mod tests {
    use super::*;
    fn temporary() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        temp
    }
    #[test]
    fn old_state_and_foreign_marker_are_refused_without_reading_or_rewriting_old_data() {
        for name in [
            "domain.sqlite3",
            "custody.sqlite3",
            "operator.token",
            "fleet-runtime.json",
            "task_graphs.json",
            "agent-driver.json",
            "development-driver.json",
            "matrix.access_token",
            "matrix.appservice_token",
            "approval.access_token",
            "palpo-transport.json",
            "palpo.machine_token",
        ] {
            let temp = temporary();
            let old = temp.path().join(name);
            std::fs::write(&old, b"legacy sentinel").unwrap();
            assert!(matches!(initialize(temp.path()), Err(Error::State)));
            assert_eq!(std::fs::read(old).unwrap(), b"legacy sentinel");
            assert!(!temp.path().join(MARKER).exists());
            assert!(!temp.path().join(LOCK).exists());
        }
        let temp = temporary();
        std::fs::write(temp.path().join(MARKER), b"foreign").unwrap();
        assert!(initialize(temp.path()).is_err());
    }
    #[test]
    fn fresh_format_is_reopenable_and_has_one_process_owner() {
        let temp = temporary();
        // Let production create the private directory, including its Windows DACL.
        let path = temp.path().join("state");
        let state = prepare(&path).unwrap();
        assert_eq!(
            std::fs::read(state.path.join(MARKER)).unwrap(),
            MARKER_BYTES
        );
        assert!(matches!(prepare(&path), Err(Error::AlreadyRunning)));
        drop(state);
        assert!(initialize(&path).is_ok());
        assert!(!path.join("domain.sqlite3").exists());
        assert!(!path.join("operator.token").exists());
    }

    #[test]
    fn credential_migration_owner_host_restart_retains_exact_interrupted_journal_temp() {
        let temp = temporary();
        let path = temp.path().join("state");
        drop(prepare(&path).unwrap());
        let name =
            ".server-login-revocations.json.vault-journal-0123456789abcdef0123456789abcdef.tmp";
        let journal = path.join(name);
        let bytes = b"{\"version\":1,\"incomplete_journal\":true}";
        hagency_store::private::write_new(&journal, bytes).unwrap();
        let original = hagency_store::private::open(&journal, false).unwrap();
        // This is the actual production restart admission. A partial journal
        // temporary is retained, never parsed as credentials or imported.
        drop(prepare(&path).unwrap());
        let reopened = hagency_store::private::open(&journal, false).unwrap();
        assert!(hagency_platform::same_file(&original, &reopened).unwrap());
        assert_eq!(std::fs::read(&journal).unwrap(), bytes);
        assert!(!path.join("server-login-revocations.json").exists());
        assert!(
            !path
                .join(".server-login-revocations.json.vault-journal")
                .exists()
        );

        for name in [
            ".other.json.vault-journal-0123456789abcdef0123456789abcdef.tmp",
            ".server-login-revocations.json.vault-journal-0123456789abcdef.tmp",
            ".server-login-revocations.json.vault-journal-0123456789abcdef0123456789abcdeg.tmp",
            ".server-login-revocations.json.vault-journal-0123456789ABCDEF0123456789ABCDEF.tmp",
            ".server-login-revocations.json.vault-unrecognized-0123456789abcdef0123456789abcdef.tmp",
        ] {
            // Isolate each namespace: case-insensitive filesystems would alias
            // the uppercase-ID negative with the retained lowercase witness.
            let foreign_temp = temporary();
            let foreign_root = foreign_temp.path().join("state");
            drop(prepare(&foreign_root).unwrap());
            let foreign = foreign_root.join(name);
            hagency_store::private::write_new(&foreign, b"foreign sentinel").unwrap();
            assert!(
                matches!(prepare(&foreign_root), Err(Error::State)),
                "{name}"
            );
            assert_eq!(std::fs::read(&foreign).unwrap(), b"foreign sentinel");
        }
    }
}
