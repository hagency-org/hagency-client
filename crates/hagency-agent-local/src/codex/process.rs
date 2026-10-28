//! Programming children retain guardian/job custody through initialization and cancellation.
use super::*;
pub type Reader = Box<dyn AsyncRead + Unpin + Send>;
pub type Writer = Box<dyn AsyncWrite + Unpin + Send>;
pub(crate) enum Child {
    Direct(Box<crate::process_cleanup::RetainedChild>),
    Supervised {
        owner: Box<hagency_platform::SupervisedProcess>,
        stderr: tokio::task::JoinHandle<()>,
    },
}
impl Child {
    pub(crate) async fn stop(&mut self) -> Result<()> {
        match self {
            Self::Direct(child) => {
                if child.try_wait()?.is_none() {
                    child.kill().await?;
                }
                Ok(())
            }
            Self::Supervised { owner, stderr } => {
                let report = owner.stop(Duration::from_secs(3))?;
                stderr.abort();
                if !report.scope.whole_tree_stopped {
                    return Err(Error::Unknown);
                }
                Ok(())
            }
        }
    }
}
pub(super) fn supervised(
    command: &tokio::process::Command,
    profile: &Profile,
    guardian: &std::path::Path,
) -> Result<Process> {
    let launch = hagency_platform::Launch {
        executable: profile.executable.clone(),
        arguments: command
            .as_std()
            .get_args()
            .map(|v| v.to_os_string())
            .collect(),
        directory: profile.cwd.clone(),
        environment: command
            .as_std()
            .get_envs()
            .filter_map(|(k, v)| v.map(|v| (k.to_os_string(), v.to_os_string())))
            .collect(),
        require_crash_containment: false,
    };
    let (owner, pipes) = hagency_platform::SupervisedProcess::spawn_piped(guardian, &launch)?;
    #[cfg(unix)]
    let (writer, reader, stderr) = {
        let (stdin, stdout, stderr) = pipes.into_parts();
        (
            tokio::net::unix::pipe::Sender::from_owned_fd(stdin)?,
            tokio::net::unix::pipe::Receiver::from_owned_fd(stdout)?,
            tokio::net::unix::pipe::Receiver::from_owned_fd(stderr)?,
        )
    };
    #[cfg(windows)]
    let (writer, reader, stderr) = {
        let (stdin, stdout, stderr) = pipes.into_async_parts()?;
        (stdin, stdout, stderr)
    };
    Ok(Process {
        session: Session::new(Box::new(reader) as Reader, Box::new(writer) as Writer),
        child: Child::Supervised {
            owner: Box::new(owner),
            stderr: tokio::spawn(async move {
                let mut stderr = stderr;
                let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
            }),
        },
    })
}

impl Drop for Child {
    fn drop(&mut self) {
        if let Self::Supervised { owner, stderr } = self {
            let _ = owner.stop(Duration::from_secs(3));
            stderr.abort();
        }
    }
}
