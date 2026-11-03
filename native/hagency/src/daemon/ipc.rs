//! Private Unix-socket transport for the owner daemon.
//!
//! The socket lives in the daemon's 0700 home with mode 0600. Every accepted
//! peer must run as the daemon's effective user; a client likewise refuses a
//! socket that is not owned by its own user, has group/other permissions, or
//! whose server process runs as someone else. Connection count, frame size,
//! requests per connection and every read/write are bounded. A local
//! connection grants no owner identity: the dispatcher still requires the
//! exact adopted owner scope on every request.
use super::{
    dispatch::{Controller, Runtime},
    protocol::{
        self, API_VERSION, DaemonError, FrameError, Reply, Request, decode_request, errors,
        read_frame, write_frame, read_reply, write_reply,
    },
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::net::{UnixListener, UnixStream};

pub const SOCKET: &str = "control.sock";
pub const MAX_CONNECTIONS: usize = 8;
pub const MAX_REQUESTS_PER_CONNECTION: usize = 32;
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// A pause waits for the admission barrier (bounded below this); a stop
/// replies once its drain was durably accepted.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(45);

pub fn effective_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("the daemon socket is not private to this user")]
    Unsafe,
    #[error("the daemon is not running")]
    Unavailable,
    #[error("the daemon peer is not this user")]
    PeerRejected,
    /// The request may have been applied. Retry with the same request ID.
    #[error("no reply was received; the outcome is unknown")]
    Uncertain,
    #[error("the daemon reply was invalid")]
    Protocol,
}

/// Validates or replaces a stale socket and binds a new private one. The
/// caller must hold the daemon home lock, so no other daemon serves it.
pub fn bind(home: &Path) -> Result<UnixListener, IpcError> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    let path = home.join(SOCKET);
    match std::fs::symlink_metadata(&path) {
        Ok(m) => {
            if !m.file_type().is_socket() || m.uid() != effective_uid() {
                return Err(IpcError::Unsafe);
            }
            std::fs::remove_file(&path).map_err(|_| IpcError::Unsafe)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(IpcError::Unsafe),
    }
    // Bind under a restrictive umask window: chmod follows immediately and
    // the directory itself is 0700, so no other user can reach the socket.
    let listener = UnixListener::bind(&path).map_err(|_| IpcError::Unavailable)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|_| IpcError::Unsafe)?;
    Ok(listener)
}

/// Running listener. Dropping it stops accepting and removes the socket.
pub struct Server {
    task: Option<tokio::task::JoinHandle<()>>,
    path: PathBuf,
}

impl Server {
    pub fn start<R: Runtime>(
        home: &Path,
        controller: Arc<Controller<R>>,
    ) -> Result<Self, IpcError> {
        let listener = bind(home)?;
        Ok(Self {
            task: Some(tokio::spawn(serve(listener, controller, effective_uid()))),
            path: home.join(SOCKET),
        })
    }
    pub async fn stop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

pub(crate) async fn serve<R: Runtime>(
    listener: UnixListener,
    controller: Arc<Controller<R>>,
    expected_uid: u32,
) {
    let permits = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    let mut connections = tokio::task::JoinSet::new();
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            Some(_) = connections.join_next(), if !connections.is_empty() => continue,
        };
        let Ok((mut stream, _)) = accepted else {
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };
        // Peer identity first: another user receives no bytes at all.
        if stream
            .peer_cred()
            .ok()
            .is_none_or(|peer| peer.uid() != expected_uid)
        {
            tracing::warn!(code = "daemon_peer_rejected", "Local control peer rejected");
            continue;
        }
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            // Inline and short: refusals cannot accumulate as tasks.
            let _ = refuse(&mut stream, errors::BUSY, Duration::from_millis(200)).await;
            continue;
        };
        let controller = controller.clone();
        connections.spawn(async move {
            let _permit = permit;
            connection(stream, controller).await;
        });
    }
}

async fn refuse(
    stream: &mut UnixStream,
    error: DaemonError,
    within: Duration,
) -> Result<(), FrameError> {
    let reply = serde_json::to_vec(&Reply::err("", 0, error)).unwrap_or_default();
    tokio::time::timeout(within, write_frame(stream, &reply))
        .await
        .map_err(|_| FrameError::Closed)?
}

async fn connection<R: Runtime>(mut stream: UnixStream, controller: Arc<Controller<R>>) {
    for _ in 0..MAX_REQUESTS_PER_CONNECTION {
        let payload = match tokio::time::timeout(IDLE_TIMEOUT, read_frame(&mut stream)).await {
            Ok(Ok(payload)) => payload,
            Ok(Err(FrameError::TooLarge)) => {
                let _ = refuse(&mut stream, errors::FRAME_TOO_LARGE, WRITE_TIMEOUT).await;
                return;
            }
            _ => return,
        };
        // The command future runs to completion even if this peer leaves:
        // a mutation is never cancelled by a controller closing.
        let reply = match decode_request(&payload) {
            Ok(request) => controller.handle(request).await,
            Err((id, error)) => Reply::err(&id, controller.revision(), error),
        };
        let Ok(mut bytes) = serde_json::to_vec(&reply) else {
            return;
        };
        if bytes.len()>protocol::MAX_REPLY_FRAME {
            bytes=serde_json::to_vec(&Reply::err(&reply.request_id,controller.revision(),errors::FRAME_TOO_LARGE)).unwrap_or_default();
        }
        if !matches!(
            tokio::time::timeout(WRITE_TIMEOUT, write_reply(&mut stream, &bytes)).await,
            Ok(Ok(()))
        ) {
            return;
        }
    }
}

/// Controller-side connector. Holds one private connection.
pub struct DaemonClient {
    stream: UnixStream,
}

pub use super::new_request_id;

impl DaemonClient {
    pub async fn connect(home: &Path) -> Result<Self, IpcError> {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let uid = effective_uid();
        let dir = std::fs::symlink_metadata(home).map_err(|_| IpcError::Unavailable)?;
        if !dir.is_dir() || dir.uid() != uid || dir.mode() & 0o077 != 0 {
            return Err(IpcError::Unsafe);
        }
        let path = home.join(SOCKET);
        let socket = std::fs::symlink_metadata(&path).map_err(|_| IpcError::Unavailable)?;
        if !socket.file_type().is_socket() || socket.uid() != uid || socket.mode() & 0o077 != 0 {
            return Err(IpcError::Unsafe);
        }
        let stream = tokio::time::timeout(Duration::from_secs(3), UnixStream::connect(&path))
            .await
            .map_err(|_| IpcError::Unavailable)?
            .map_err(|_| IpcError::Unavailable)?;
        if stream
            .peer_cred()
            .ok()
            .is_none_or(|peer| peer.uid() != uid)
        {
            return Err(IpcError::PeerRejected);
        }
        Ok(Self { stream })
    }

    /// Send one request and await its reply. A transport failure after the
    /// request was written is `Uncertain`: retry with the same request ID.
    pub async fn call(&mut self, request: &Request) -> Result<Reply, IpcError> {
        let bytes = serde_json::to_vec(request).map_err(|_| IpcError::Protocol)?;
        if bytes.len() > protocol::MAX_FRAME {
            return Err(IpcError::Protocol);
        }
        match tokio::time::timeout(WRITE_TIMEOUT, write_frame(&mut self.stream, &bytes)).await {
            Ok(Ok(())) => {}
            // A partial write may still have delivered a whole frame.
            _ => return Err(IpcError::Uncertain),
        }
        let payload = match tokio::time::timeout(CALL_TIMEOUT, read_reply(&mut self.stream)).await
        {
            Ok(Ok(payload)) => payload,
            _ => return Err(IpcError::Uncertain),
        };
        let reply: Reply = serde_json::from_slice(&payload).map_err(|_| IpcError::Protocol)?;
        if reply.version != API_VERSION {
            return Err(IpcError::Protocol);
        }
        // An error for an unparsable request carries an empty ID.
        if reply.request_id != request.request_id
            && !(reply.request_id.is_empty() && reply.error.is_some())
        {
            return Err(IpcError::Protocol);
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::super::dispatch::tests::{Probe, controller};
    use super::super::protocol::{Command, OutcomeState};
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn private_dir() -> tempfile::TempDir {
        // Short path: Unix socket paths are limited to ~104 bytes on macOS.
        let temp = tempfile::Builder::new().prefix("hd").tempdir_in("/tmp").unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        temp
    }

    fn req(id: &str, command: Command) -> Request {
        Request::new(id, Some("@o:s".into()), command)
    }

    #[tokio::test]
    async fn socket_is_private_and_end_to_end_commands_work() {
        let temp = private_dir();
        let probe = Probe::new(Some("@o:s"));
        let c = controller(temp.path(), probe.clone());
        c.boot().await;
        let mut server = Server::start(temp.path(), c).unwrap();
        let meta = std::fs::symlink_metadata(temp.path().join(SOCKET)).unwrap();
        assert_eq!(meta.mode() & 0o777, 0o600);
        assert_eq!(meta.uid(), effective_uid());
        let mut client = DaemonClient::connect(temp.path()).await.unwrap();
        let reply = client.call(&req("pause-1", Command::PauseHosting)).await.unwrap();
        assert_eq!(reply.outcome().unwrap().state, OutcomeState::Completed);
        // Retried over a new connection, the same ID replays the same reply.
        let mut again = DaemonClient::connect(temp.path()).await.unwrap();
        assert_eq!(again.call(&req("pause-1", Command::PauseHosting)).await.unwrap(), reply);
        let snapshot = again
            .call(&req("s", Command::Snapshot { period: None, usage_filter:None }))
            .await
            .unwrap()
            .snapshot()
            .unwrap();
        assert_eq!(snapshot.owner.unwrap().user_id, "@o:s");
        server.stop().await;
        assert!(!temp.path().join(SOCKET).exists());
        assert!(matches!(
            DaemonClient::connect(temp.path()).await,
            Err(IpcError::Unavailable)
        ));
    }

    #[tokio::test]
    async fn client_refuses_shared_socket_or_directory() {
        let temp = private_dir();
        let c = controller(temp.path(), Probe::new(Some("@o:s")));
        let _server = Server::start(temp.path(), c).unwrap();
        let socket = temp.path().join(SOCKET);
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(matches!(DaemonClient::connect(temp.path()).await, Err(IpcError::Unsafe)));
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(DaemonClient::connect(temp.path()).await, Err(IpcError::Unsafe)));
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(DaemonClient::connect(temp.path()).await.is_ok());
    }

    #[tokio::test]
    async fn bind_refuses_a_non_socket_in_place_of_the_socket() {
        let temp = private_dir();
        std::fs::write(temp.path().join(SOCKET), b"not a socket").unwrap();
        assert!(matches!(bind(temp.path()), Err(IpcError::Unsafe)));
        assert_eq!(std::fs::read(temp.path().join(SOCKET)).unwrap(), b"not a socket");
    }

    #[tokio::test]
    async fn peer_of_another_user_receives_nothing() {
        let temp = private_dir();
        let c = controller(temp.path(), Probe::new(Some("@o:s")));
        let listener = bind(temp.path()).unwrap();
        // The server expects a different UID than this test process has.
        let task = tokio::spawn(serve(listener, c, effective_uid().wrapping_add(1)));
        let mut stream = UnixStream::connect(temp.path().join(SOCKET)).await.unwrap();
        let bytes = serde_json::to_vec(&req("x", Command::PauseHosting)).unwrap();
        let _ = write_frame(&mut stream, &bytes).await;
        let mut buf = Vec::new();
        let read = tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut buf)).await;
        assert!(matches!(read, Ok(Ok(0)) | Ok(Err(_))), "{read:?}");
        task.abort();
    }

    #[tokio::test]
    async fn oversized_and_malformed_frames_are_bounded() {
        let temp = private_dir();
        let c = controller(temp.path(), Probe::new(Some("@o:s")));
        let _server = Server::start(temp.path(), c).unwrap();
        let mut client = DaemonClient::connect(temp.path()).await.unwrap();
        // Malformed JSON: structured error, connection stays usable.
        write_frame(&mut client.stream, b"{oops").await.unwrap();
        let reply: Reply = serde_json::from_slice(&read_frame(&mut client.stream).await.unwrap()).unwrap();
        assert_eq!(reply.error.unwrap().code, "invalid_request");
        let wrong_version = br#"{"version":9,"requestId":"v","command":{"type":"hello"}}"#;
        write_frame(&mut client.stream, wrong_version).await.unwrap();
        let reply: Reply = serde_json::from_slice(&read_frame(&mut client.stream).await.unwrap()).unwrap();
        assert_eq!(reply.error.unwrap().code, "unsupported_version");
        // Oversized header: refused and closed without reading the payload.
        client
            .stream
            .write_all(&((protocol::MAX_FRAME as u32) + 1).to_be_bytes())
            .await
            .unwrap();
        let reply: Reply = serde_json::from_slice(&read_frame(&mut client.stream).await.unwrap()).unwrap();
        assert_eq!(reply.error.unwrap().code, "frame_too_large");
        assert!(matches!(read_frame(&mut client.stream).await, Err(FrameError::Closed) | Err(FrameError::Io(_))));
    }

    #[tokio::test]
    async fn connection_and_request_counts_are_bounded() {
        let temp = private_dir();
        let c = controller(temp.path(), Probe::new(Some("@o:s")));
        let _server = Server::start(temp.path(), c).unwrap();
        let mut held = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            let mut client = DaemonClient::connect(temp.path()).await.unwrap();
            // Complete one round trip so the server has admitted it.
            client.call(&req(&new_request_id(), Command::Hello)).await.unwrap();
            held.push(client);
        }
        let mut extra = DaemonClient::connect(temp.path()).await.unwrap();
        let refused: Reply = serde_json::from_slice(&read_frame(&mut extra.stream).await.unwrap()).unwrap();
        assert_eq!(refused.error.unwrap().code, "busy");
        drop(held);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut client = DaemonClient::connect(temp.path()).await.unwrap();
        for i in 1..MAX_REQUESTS_PER_CONNECTION {
            client.call(&req(&format!("h{i}"), Command::Hello)).await.unwrap();
        }
        client.call(&req("last", Command::Hello)).await.unwrap();
        // The connection closes after its request budget.
        assert!(matches!(
            client.call(&req("over", Command::Hello)).await,
            Err(IpcError::Uncertain)
        ));
    }

    #[test]
    fn request_ids_are_valid_and_distinct() {
        let a = new_request_id();
        assert!(protocol::valid_request_id(&a));
        assert_ne!(a, new_request_id());
    }
}
