//! Standalone owner client. No legacy App, domain repository, Matrix collector
//! or Fleet bootstrap is constructed by this production entry point.
use crate::console::Console;
use salvo::prelude::*;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
const MARKER: &str = "hagency-client-owned-v1.json";
const MARKER_BYTES: &[u8] = b"{\"format\":\"hagency-owned-agent-client\",\"version\":1}\n";
const LOCK: &str = ".owner-client.lock";
#[cfg(unix)]
const ACCESS_SOCKET: &str = "owner-console.sock";
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("use a fresh private owner-client state directory; legacy or foreign data is refused")]
    State,
    #[error("another owner-client process is using this state directory")]
    AlreadyRunning,
    #[error("use a nonzero loopback listen address")]
    Address,
    #[error("an owner console build is required")]
    Console {
        field: &'static str,
        fix: &'static str,
    },
    #[error("owner-client HTTP listener failed")]
    Server,
}
fn console_error() -> Error {
    Error::Console {
        field: "--console-assets",
        fix: "build a validated private owner console bundle; use its real directory, never a final-directory symlink",
    }
}
struct State {
    path: PathBuf,
    _lock: std::fs::File,
    directory: std::fs::File,
}
fn allowed_entry(name: &str) -> bool {
    matches!(name,MARKER|LOCK|"server-login.json"|"server-login-sdk-resume.json"|"server-login-revocations.json"|"server-login-profiles.json"|"owned-agent-owners"|"owner-console.sock")
        || (cfg!(feature = "resource-adoption") && name == "adopted-management-initializer-v1.json")
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
    let directory = cap_std::fs::Dir::open_ambient_dir(&path, cap_std::ambient_authority())
        .map_err(|_| Error::State)?
        .into_std_file();
    Ok(State {
        path,
        _lock: lock,
        directory,
    })
}
/// Initializes only the new format, and never imports old SQLite or credentials.
pub fn initialize(path: &Path) -> Result<PathBuf, Error> {
    Ok(prepare(path)?.path)
}
/// A reduction-only native operation on an already initialized, exclusively
/// owned root. Does not construct a console, database, listener or new root.
pub(crate) fn with_existing_state<T>(
    path: &Path,
    action: impl FnOnce(&Path) -> T,
) -> Result<Option<T>, Error> {
    if !path.is_absolute() {
        return Err(Error::State);
    }
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Error::State),
        Ok(m) if !m.is_dir() || m.file_type().is_symlink() => return Err(Error::State),
        Ok(_) => (),
    }
    if hagency_store::private::read_secret(&path.join(MARKER)).map_err(|_| Error::State)?
        != MARKER_BYTES
    {
        return Err(Error::State);
    }
    let state = prepare(path)?;
    Ok(Some(action(&state.path)))
}
#[derive(Clone)]
pub struct OwnerHost {
    pub(crate) console: Console,
    pub(crate) authority: String,
    state: Arc<State>,
}
impl OwnerHost {
    pub fn access_link(&self) -> Result<String, Error> {
        let ticket = self
            .console
            .owner_access_ticket()
            .map_err(|_| console_error())?;
        Ok(format!(
            "http://{}/console/login#access={ticket}",
            self.authority
        ))
    }
    pub fn open(state: &Path, address: SocketAddr, assets: Option<&Path>) -> Result<Self, Error> {
        if !address.ip().is_loopback() || address.port() == 0 {
            return Err(Error::Address);
        }
        let state = prepare(state)?;
        let console = match assets {
            Some(path) => Console::load_owner_with_state(path, &state.path),
            None => Console::embedded_owner_with_state(&state.path),
        }
        .map_err(|_| console_error())?;
        Ok(Self {
            console,
            authority: address.to_string(),
            state: Arc::new(state),
        })
    }
    /// In-process native management. No web assets are loaded or presented.
    pub(crate) fn open_native(state: &Path, address: SocketAddr) -> Result<Self, Error> {
        if !address.ip().is_loopback() || address.port() == 0 {
            return Err(Error::Address);
        }
        Self::native_with_authority(state, address.to_string())
    }
    /// The embedding application's Matrix SDK owns authentication. This host
    /// has no socket or browser callback. Its reserved authority is used only
    /// by the in-process request adapter and can never be passed to `serve`.
    pub(crate) fn open_embedded(state: &Path) -> Result<Self, Error> {
        Self::native_with_authority(state, "hagency-native.invalid".into())
    }
    fn native_with_authority(state: &Path, authority: String) -> Result<Self, Error> {
        let state = prepare(state)?;
        let console = Console::native_with_state(&state.path).map_err(|_| console_error())?;
        Ok(Self {
            console,
            authority,
            state: Arc::new(state),
        })
    }
    pub fn state_directory(&self) -> &Path {
        &self.state.path
    }
    /// Retaining this host is root-lock custody only while the physical root
    /// and lock path still identify the original private handles.
    pub(crate) fn check_state_lock(&self) -> Result<(), Error> {
        use cap_fs_ext::DirExt;
        let parent = cap_std::fs::Dir::open_ambient_dir(
            self.state.path.parent().ok_or(Error::State)?,
            cap_std::ambient_authority(),
        )
        .map_err(|_| Error::State)?;
        let current = parent
            .open_dir_nofollow(self.state.path.file_name().ok_or(Error::State)?)
            .map_err(|_| Error::State)?
            .into_std_file();
        let lock = hagency_store::private::open(&self.state.path.join(LOCK), false)
            .map_err(|_| Error::State)?;
        if !hagency_platform::same_directory(&self.state.directory, &current)
            .map_err(|_| Error::State)?
            || !hagency_platform::same_file(&self.state._lock, &lock).map_err(|_| Error::State)?
        {
            return Err(Error::State);
        }
        hagency_store::private::check_handle(&current).map_err(|_| Error::State)?;
        hagency_store::private::check_handle(&lock).map_err(|_| Error::State)
    }
    pub fn router(self) -> Router {
        Router::new()
            .hoop(self)
            .push(Router::with_path("health").get(health))
            .push(Router::with_path("ready").get(health))
            .push(crate::console::owner_router())
    }
    pub async fn serve(
        self,
        address: SocketAddr,
        shutdown: &hagency_matrix::CancellationToken,
    ) -> Result<(), Error> {
        if self.authority != address.to_string() {
            return Err(Error::Address);
        }
        #[cfg(unix)]
        let mut access = access_listener(self.clone())?;
        let acceptor = TcpListener::new(address)
            .try_bind()
            .await
            .map_err(|_| Error::Server)?;
        let server = Server::new(acceptor).max_connections(64);
        let handle = server.handle();
        let mut serving = Box::pin(server.try_serve(self.clone().router()));
        let result = tokio::select! { result=&mut serving=>result.map_err(|_|Error::Server),_=shutdown.cancelled()=>Ok(())};
        self.console.retire();
        self.console.stop_owned_runtimes().await;
        self.console.stop_owner_provider().await;
        #[cfg(unix)]
        {
            access.stop().await;
        }
        handle.stop_graceful(Some(Duration::from_secs(5)));
        if result.is_ok() {
            serving.await.map_err(|_| Error::Server)?;
        }
        result
    }
}

#[cfg(unix)]
struct AccessService {
    task: Option<tokio::task::JoinHandle<()>>,
    path: PathBuf,
}
#[cfg(unix)]
impl AccessService {
    async fn stop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}
#[cfg(unix)]
impl Drop for AccessService {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}
#[cfg(unix)]
fn access_listener(host: OwnerHost) -> Result<AccessService, Error> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    let path = host.state.path.join(ACCESS_SOCKET);
    if let Ok(metadata) = std::fs::symlink_metadata(&path) {
        if !metadata.file_type().is_socket() || metadata.mode() & 0o077 != 0 {
            return Err(Error::State);
        }
        std::fs::remove_file(&path).map_err(|_| Error::State)?;
    }
    let listener = tokio::net::UnixListener::bind(&path).map_err(|_| Error::Server)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|_| Error::State)?;
    let uid = std::fs::metadata(&host.state.path)
        .map_err(|_| Error::State)?
        .uid();
    let task = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            if socket.peer_cred().ok().is_none_or(|peer| peer.uid() != uid) {
                continue;
            }
            let _ = tokio::time::timeout(Duration::from_secs(2), async {
                let mut request = [0; 8];
                socket.read_exact(&mut request).await?;
                if &request != b"ACCESS1\n" {
                    return Ok::<_, std::io::Error>(());
                }
                if let Ok(url) = host.access_link() {
                    let response =
                        serde_json::to_vec(&serde_json::json!({"url":url})).expect("string JSON");
                    socket.write_all(&response).await?;
                    socket.shutdown().await?;
                }
                Ok(())
            })
            .await;
        }
    });
    Ok(AccessService {
        task: Some(task),
        path,
    })
}
/// Local filesystem/OS IPC, not an HTTP privilege issuer. The caller must own
/// the private new-format state and the socket; a link still needs Pasion login.
#[cfg(unix)]
pub async fn request_access(path: &Path) -> Result<String, Error> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    hagency_store::private::directory(path).map_err(|_| Error::State)?;
    let marker =
        hagency_store::private::read_secret(&path.join(MARKER)).map_err(|_| Error::State)?;
    if marker != MARKER_BYTES {
        return Err(Error::State);
    }
    let uid = std::fs::metadata(path).map_err(|_| Error::State)?.uid();
    let socket = path.join(ACCESS_SOCKET);
    let metadata = std::fs::symlink_metadata(&socket).map_err(|_| Error::Server)?;
    if !metadata.file_type().is_socket() || metadata.mode() & 0o077 != 0 || metadata.uid() != uid {
        return Err(Error::State);
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut socket = tokio::net::UnixStream::connect(socket)
            .await
            .map_err(|_| Error::Server)?;
        if socket.peer_cred().ok().is_none_or(|peer| peer.uid() != uid) {
            return Err(Error::State);
        }
        socket
            .write_all(b"ACCESS1\n")
            .await
            .map_err(|_| Error::Server)?;
        let mut response = Vec::new();
        socket
            .take(2048)
            .read_to_end(&mut response)
            .await
            .map_err(|_| Error::Server)?;
        let value: serde_json::Value =
            serde_json::from_slice(&response).map_err(|_| Error::Server)?;
        let url = value["url"].as_str().ok_or(Error::Server)?;
        let parsed = reqwest::Url::parse(url).map_err(|_| Error::Server)?;
        if parsed.scheme() != "http"
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.path() != "/console/login"
            || parsed.query().is_some()
            || !parsed
                .fragment()
                .is_some_and(|f| f.starts_with("access=") && f.len() <= 128)
            || !parsed
                .host_str()
                .and_then(|s| s.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok())
                .is_some_and(|ip| ip.is_loopback())
        {
            return Err(Error::Server);
        }
        Ok(url.into())
    })
    .await
    .map_err(|_| Error::Server)?
}
#[cfg(not(unix))]
pub async fn request_access(_path: &Path) -> Result<String, Error> {
    Err(console_error())
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
        let temp = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
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
        let state = prepare(temp.path()).unwrap();
        assert_eq!(
            std::fs::read(state.path.join(MARKER)).unwrap(),
            MARKER_BYTES
        );
        assert!(matches!(prepare(temp.path()), Err(Error::AlreadyRunning)));
        drop(state);
        assert!(initialize(temp.path()).is_ok());
        assert!(!temp.path().join("domain.sqlite3").exists());
        assert!(!temp.path().join("operator.token").exists());
    }
    fn bundle(root: &Path, legacy: bool) -> PathBuf {
        use sha2::{Digest, Sha256};
        use std::io::Write;
        let directory = root.join(if legacy {
            "legacy-assets"
        } else {
            "owner-assets"
        });
        hagency_store::private::directory(&directory).unwrap();
        let mut entries = Vec::new();
        let mut paths = vec![
            "index.html",
            "login/index.html",
            "agents-owned/index.html",
            "projects/index.html",
            "projects/new/index.html",
        ];
        if legacy {
            paths.push("usage/index.html");
        }
        for path in paths {
            let file = directory.join(path);
            if let Some(parent) = file.parent() {
                hagency_store::private::directory(parent).unwrap();
            }
            let bytes = b"<!doctype html><html><body>owner console fixture</body></html>";
            let mut output = hagency_store::private::open(&file, true).unwrap();
            output.write_all(bytes).unwrap();
            entries.push(serde_json::json!({"path":path,"mime":"text/html; charset=utf-8","size":bytes.len(),"sha256":format!("{:x}",Sha256::digest(bytes))}));
        }
        let mut manifest =
            hagency_store::private::open(&directory.join("manifest.json"), true).unwrap();
        manifest
            .write_all(
                &serde_json::to_vec(&serde_json::json!({"version":1,"assets":entries})).unwrap(),
            )
            .unwrap();
        directory
    }
    #[tokio::test]
    async fn owner_assets_and_router_refuse_every_retired_surface() {
        let temp = temporary();
        let assets = bundle(temp.path(), false);
        let state = temp.path().join("state");
        let address = "127.0.0.1:13300".parse().unwrap();
        let host = OwnerHost::open(&state, address, Some(&assets)).unwrap();
        let service = Service::new(host.clone().router());
        for path in ["/console/", "/console/login/", "/console/agents-owned/"] {
            let response = salvo::test::TestClient::get(format!("http://{address}{path}"))
                .add_header("host", address.to_string(), true)
                .add_header("sec-fetch-site", "same-origin", true)
                .send(&service)
                .await;
            assert_eq!(response.status_code, Some(StatusCode::OK), "{path}");
        }
        for path in [
            "/api/native/v1/resources",
            "/api/native/v1/custody",
            "/api/native/v1/fleet",
            "/api/native/v1/console/access",
            "/runner",
            "/console/usage/",
            "/console/resources/",
            "/console/engagements/",
            "/console/api/engagements",
            "/console/api/accounts",
            "/console/api/approvals",
            "/console/api/resource-configuration",
        ] {
            let response = salvo::test::TestClient::get(format!("http://{address}{path}"))
                .add_header("host", address.to_string(), true)
                .add_header("sec-fetch-site", "same-origin", true)
                .send(&service)
                .await;
            assert_eq!(
                response.status_code,
                Some(StatusCode::NOT_FOUND),
                "retired route still resolved: {path}"
            );
        }
        assert!(!state.join("domain.sqlite3").exists());
        assert!(!state.join("operator.token").exists());
        assert!(!state.join("task_graphs.json").exists());
        host.console.retire();
        host.console.stop_owned_runtimes().await;
        host.console.stop_owner_provider().await;
        let legacy = bundle(temp.path(), true);
        assert!(matches!(
            OwnerHost::open(&temp.path().join("rejected-state"), address, Some(&legacy)),
            Err(Error::Console { .. })
        ));
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn private_ipc_accepts_ipv6_loopback_access_link() {
        let temp = temporary();
        let assets = bundle(temp.path(), false);
        let state = temp.path().join("state");
        let address = "[::1]:13300".parse().unwrap();
        let host = OwnerHost::open(&state, address, Some(&assets)).unwrap();
        let mut ipc = access_listener(host.clone()).unwrap();
        let link = request_access(&state).await.unwrap();
        assert!(link.starts_with("http://[::1]:13300/console/login#access="));
        let ticket = reqwest::Url::parse(&link)
            .unwrap()
            .fragment()
            .unwrap()
            .strip_prefix("access=")
            .unwrap()
            .to_owned();
        let service = Service::new(host.clone().router());
        let response = salvo::test::TestClient::post(format!("http://{address}/console/session"))
            .add_header("host", address.to_string(), true)
            .add_header("sec-fetch-site", "same-origin", true)
            .add_header("origin", format!("http://{address}"), true)
            .json(&serde_json::json!({"ticket":ticket}))
            .send(&service)
            .await;
        assert_eq!(response.status_code, Some(StatusCode::OK));
        ipc.stop().await;
        host.console.retire();
        host.console.stop_owner_provider().await;
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn private_ipc_mints_finite_local_gate_but_not_pasion_authorization() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let temp = temporary();
        let assets = bundle(temp.path(), false);
        let state = temp.path().join("state");
        let address = "127.0.0.1:13300".parse().unwrap();
        let host = OwnerHost::open(&state, address, Some(&assets)).unwrap();
        let mut ipc = access_listener(host.clone()).unwrap();
        let mut invalid = tokio::net::UnixStream::connect(state.join(ACCESS_SOCKET))
            .await
            .unwrap();
        invalid.write_all(b"BADCODE\n").await.unwrap();
        let mut empty = Vec::new();
        invalid.read_to_end(&mut empty).await.unwrap();
        assert!(empty.is_empty());
        let link = request_access(&state).await.unwrap();
        let parsed = reqwest::Url::parse(&link).unwrap();
        let ticket = parsed.fragment().unwrap().strip_prefix("access=").unwrap();
        let service = Service::new(host.clone().router());
        let exchange = salvo::test::TestClient::post(format!("http://{address}/console/session"))
            .add_header("host", address.to_string(), true)
            .add_header("sec-fetch-site", "same-origin", true)
            .add_header("origin", format!("http://{address}"), true)
            .json(&serde_json::json!({"ticket":ticket}))
            .send(&service)
            .await;
        assert_eq!(exchange.status_code, Some(StatusCode::OK));
        let cookie = exchange
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        let response =
            salvo::test::TestClient::get(format!("http://{address}/console/api/owned-agents"))
                .add_header("host", address.to_string(), true)
                .add_header("sec-fetch-site", "same-origin", true)
                .add_header("cookie", cookie, true)
                .send(&service)
                .await;
        assert_ne!(response.status_code, Some(StatusCode::OK));
        ipc.stop().await;
        drop(ipc);
        assert!(!state.join(ACCESS_SOCKET).exists());
        host.console.retire();
        host.console.stop_owner_provider().await;
    }
}
