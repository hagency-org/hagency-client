//! The owner daemon's command line, `<exe> daemon <subcommand> …` (ADR-192).
//!
//! Any binary that hosts the daemon dispatches it first in a synchronous
//! `main`, right after the guardian entry and before any UI or async runtime:
//!
//! ```no_run
//! if let Some(result) = hagency::native_owner::run_guardian_if_requested() {
//!     std::process::exit(if result.is_ok() { 0 } else { 1 });
//! }
//! if let Some(status) = hagency::daemon::cli::run_if_requested() {
//!     std::process::exit(status);
//! }
//! // … start the UI
//! ```
//!
//! `daemon install` writes a LaunchAgent that runs `current_exe() daemon run
//! --home <home>`, so it works for every executable that calls this entry.
use std::ffi::{OsStr, OsString};
#[cfg(unix)]
use std::path::{Path, PathBuf};

/// The argument that selects the daemon command line.
pub const SUBCOMMAND: &str = "daemon";
/// Development builds only: an explicitly isolated installation, which the
/// LaunchAgent of such a build repeats before [`SUBCOMMAND`].
#[cfg(all(feature = "test-support", feature = "resource-adoption"))]
const ISOLATED_INSTALLATION: &str = "--hagency-isolated-installation-root";

/// Daemon subcommands. Every `--home` defaults to
/// [`crate::service::default_daemon_dir`].
#[cfg(unix)]
#[derive(clap::Subcommand, Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Run the daemon in the foreground (used by the LaunchAgent).
    Run {
        #[arg(long)]
        home: Option<PathBuf>,
    },
    /// Sign the daemon in through the browser with its own device and record
    /// the owner's one registered owner root. An active executor in that root
    /// is proved in place; otherwise a new executor device is registered.
    Enroll {
        #[arg(long)]
        home: Option<PathBuf>,
        #[arg(long)]
        server: String,
        #[arg(long)]
        owner: String,
        #[arg(long)]
        display_name: Option<String>,
        /// Cancel browser enrollment if its launching controller closes.
        #[arg(long, hide = true)]
        cancel_on_stdin_close: bool,
        #[arg(long)]
        no_open: bool,
    },
    /// Show the running daemon's owner and hosting snapshot.
    Status {
        #[arg(long)]
        home: Option<PathBuf>,
    },
    /// Stop admitting new work; running work finishes.
    Pause {
        #[arg(long)]
        home: Option<PathBuf>,
    },
    /// Resume admission, restarting hosting if it was stopped.
    Resume {
        #[arg(long)]
        home: Option<PathBuf>,
    },
    /// Drain and stop hosting; the daemon stays reachable.
    Stop {
        #[arg(long)]
        home: Option<PathBuf>,
    },
    /// Explicit owner sign-out: revoke native and SDK credentials. The owner
    /// root and its registration are kept.
    SignOut {
        #[arg(long)]
        home: Option<PathBuf>,
    },
    /// Register this executable's daemon as the user's LaunchAgent / systemd
    /// user service.
    Install {
        #[arg(long)]
        home: Option<PathBuf>,
    },
    /// Remove the daemon service; its home and enrollment are kept.
    Uninstall,
}

#[cfg(unix)]
#[derive(clap::Parser, Debug)]
#[command(about = "Hagency owner daemon")]
struct Invocation {
    #[cfg(all(feature = "test-support", feature = "resource-adoption"))]
    #[arg(
        long = "hagency-isolated-installation-root",
        global = true,
        hide = true
    )]
    isolated_installation_root: Option<PathBuf>,
    #[command(subcommand)]
    top: Top,
}

#[cfg(unix)]
#[derive(clap::Subcommand, Debug)]
enum Top {
    /// Per-user owner daemon: hosts the enrolled owner independently of any UI.
    Daemon {
        #[command(subcommand)]
        command: Command,
    },
}

/// Whether `args` (including the program name) ask for the daemon command line.
pub fn requested<S: AsRef<OsStr>>(args: &[S]) -> bool {
    let mut rest = args.iter().skip(1).map(AsRef::as_ref);
    let first = rest.next();
    #[cfg(all(feature = "test-support", feature = "resource-adoption"))]
    let first = match first {
        Some(flag) if flag == ISOLATED_INSTALLATION => rest.nth(1),
        Some(flag)
            if flag.to_str().is_some_and(|f| {
                f.starts_with(ISOLATED_INSTALLATION)
                    && f[ISOLATED_INSTALLATION.len()..].starts_with('=')
            }) =>
        {
            rest.next()
        }
        other => other,
    };
    first.is_some_and(|arg| arg == SUBCOMMAND)
}

/// `Some(exit status)` after running `daemon …` when the process was started
/// with it, `None` otherwise. Builds its own runtime: call it from a
/// synchronous `main`, before any UI or async runtime starts.
pub fn run_if_requested() -> Option<i32> {
    let args: Vec<OsString> = std::env::args_os().collect();
    if !requested(&args) {
        return None;
    }
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .try_init();
    Some(run_args(args))
}

#[cfg(unix)]
fn run_args(args: Vec<OsString>) -> i32 {
    use clap::Parser;
    let invocation = match Invocation::try_parse_from(args) {
        Ok(invocation) => invocation,
        Err(error) => {
            let _ = error.print();
            return error.exit_code();
        }
    };
    #[cfg(all(feature = "test-support", feature = "resource-adoption"))]
    if let Some(root) = &invocation.isolated_installation_root
        && let Err(code) = isolated_installation(root)
    {
        eprintln!("Error: {code}");
        return 1;
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Error: {error}");
            return 1;
        }
    };
    let Top::Daemon { command } = invocation.top;
    match runtime.block_on(run(command)) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("Error: {error}");
            1
        }
    }
}

#[cfg(not(unix))]
fn run_args(_args: Vec<OsString>) -> i32 {
    eprintln!("Error: the Hagency owner daemon is not available on this platform");
    1
}

/// The same explicit installation the host may already have selected at
/// startup; a different one is refused.
#[cfg(all(unix, feature = "test-support", feature = "resource-adoption"))]
fn isolated_installation(root: &Path) -> Result<(), String> {
    if !root.is_absolute() {
        return Err("isolated installation root must be absolute".into());
    }
    let selected = crate::native_owner::resource_activation_fixture_root();
    if selected.is_some() && selected == root.canonicalize().ok() {
        return Ok(());
    }
    crate::native_owner::initialize_resource_activation_fixture(root).map_err(|error| error.code)
}

#[cfg(unix)]
fn absolute(path: PathBuf) -> Result<PathBuf, std::io::Error> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

#[cfg(unix)]
fn daemon_home(home: Option<PathBuf>) -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(absolute(match home {
        Some(path) => path,
        None => super::host::default_home()?,
    })?)
}

#[cfg(unix)]
async fn call(
    home: &Path,
    command: super::Command,
) -> Result<super::Reply, Box<dyn std::error::Error>> {
    use super::{DaemonClient, IpcError, Request, new_request_id};
    let mut client = DaemonClient::connect(home).await?;
    let hello = client
        .call(&Request::new(new_request_id(), None, super::Command::Hello))
        .await?
        .hello()
        .map_err(|e| e.message)?;
    let request = Request::new(new_request_id(), hello.owner_id, command);
    // An uncertain reply is retried once with the same request ID.
    match client.call(&request).await {
        Ok(reply) => Ok(reply),
        Err(IpcError::Uncertain) => Ok(DaemonClient::connect(home).await?.call(&request).await?),
        Err(e) => Err(e.into()),
    }
}

/// Run one daemon subcommand to completion.
#[cfg(unix)]
pub async fn run(command: Command) -> Result<(), Box<dyn std::error::Error>> {
    use super::{Command as C, host};
    match command {
        Command::Run { home } => {
            let home = daemon_home(home)?;
            let cancel = hagency_matrix::CancellationToken::new();
            let signal = cancel.clone();
            tokio::spawn(async move {
                if let Ok(mut termination) =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                {
                    tokio::select! {_=tokio::signal::ctrl_c()=>{},_=termination.recv()=>{}}
                } else {
                    let _ = tokio::signal::ctrl_c().await;
                }
                signal.cancel();
            });
            host::run(&home, cancel).await?;
        }
        Command::Enroll {
            home,
            server,
            owner,
            display_name,
            cancel_on_stdin_close,
            no_open,
        } => {
            let home = daemon_home(home)?;
            let control = std::sync::Arc::new(host::EnrollmentControl::default());
            let enrollment = host::enroll_controlled(
                &home,
                &server,
                &owner,
                display_name,
                control.clone(),
                |url| async move {
                    println!("Sign in to enroll the Hagency owner daemon: {url}");
                    if !no_open {
                        crate::service::open_owner_link(&url);
                    }
                    Ok(())
                },
            );
            let report = if cancel_on_stdin_close {
                let (cancel, cancelled) = tokio::sync::oneshot::channel();
                // A detached OS reader must not keep the Tokio runtime alive
                // after successful enrollment while the controller owns stdin.
                std::thread::spawn(move || {
                    use std::io::Read;
                    let mut byte = [0];
                    let _ = std::io::stdin().read(&mut byte);
                    let _ = cancel.send(());
                });
                // Closing the controller cancels OAuth or detaches remote
                // enrollment. Once native registration is admitted, keep its
                // original future until cleanup is acknowledged.
                let cancel_control = control.clone();
                let watcher = tokio::spawn(async move {
                    let _ = cancelled.await;
                    cancel_control.cancel();
                });
                let report = enrollment.await;
                watcher.abort();
                report?
            } else {
                enrollment.await?
            };
            let executor = if report.existing_executor {
                "the existing executor"
            } else {
                "a new executor"
            };
            match report.pending {
                None => println!(
                    "Enrolled {} with {executor} (owner root {}). Start the daemon with `daemon run` or install it with `daemon install`.",
                    report.owner,
                    report.owner_root.display()
                ),
                Some(code) => println!(
                    "Enrolled {} but native registration is pending ({code}). The owner root may still be in use by another process.",
                    report.owner
                ),
            }
        }
        Command::Status { home } => {
            let home = daemon_home(home)?;
            match host::probe(&home).await {
                host::Presence::NotRunning => println!("Hagency owner daemon is not running."),
                host::Presence::Unsafe => {
                    return Err("the daemon socket is not private to this user".into());
                }
                host::Presence::Running(_) => {
                    let reply = call(
                        &home,
                        C::Snapshot {
                            period: None,
                            usage_filter: None,
                        },
                    )
                    .await?;
                    println!("{}", serde_json::to_string_pretty(&reply)?);
                }
            }
        }
        Command::Pause { home } => {
            let reply = call(&daemon_home(home)?, C::PauseHosting).await?;
            println!("{}", serde_json::to_string_pretty(&reply)?);
        }
        Command::Resume { home } => {
            let reply = call(&daemon_home(home)?, C::ResumeHosting).await?;
            println!("{}", serde_json::to_string_pretty(&reply)?);
        }
        Command::Stop { home } => {
            let reply = call(&daemon_home(home)?, C::StopHosting).await?;
            println!("{}", serde_json::to_string_pretty(&reply)?);
        }
        Command::SignOut { home } => {
            host::sign_out(&daemon_home(home)?).await?;
            println!("Owner signed out of the Hagency owner daemon. Its owner root is kept.");
        }
        Command::Install { home } => {
            println!("{}", crate::service::install_daemon(&daemon_home(home)?)?)
        }
        Command::Uninstall => println!("{}", crate::service::uninstall_daemon()?),
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Result<Command, clap::Error> {
        Invocation::try_parse_from(args).map(|i| {
            let Top::Daemon { command } = i.top;
            command
        })
    }

    #[test]
    fn only_a_daemon_invocation_is_dispatched() {
        for args in [
            &["rinx", "daemon", "run", "--home", "/tmp/h"][..],
            &[
                "/Applications/Rinx.app/Contents/MacOS/rinx",
                "daemon",
                "status",
            ],
            &["hagency", "daemon"],
        ] {
            assert!(requested(args), "{args:?}");
        }
        for args in [
            &["rinx"][..],
            &["rinx", "guardian"],
            &["rinx", "serve", "daemon"],
            &["rinx", "--daemon"],
            &[],
        ] {
            assert!(!requested(args), "{args:?}");
        }
        #[cfg(all(feature = "test-support", feature = "resource-adoption"))]
        {
            assert!(requested(&[
                "rinx",
                ISOLATED_INSTALLATION,
                "/tmp/isolated owner",
                "daemon",
                "run"
            ]));
            assert!(requested(&[
                "rinx",
                "--hagency-isolated-installation-root=/tmp/i",
                "daemon",
                "run"
            ]));
            assert!(!requested(&[
                "rinx",
                ISOLATED_INSTALLATION,
                "/tmp/i",
                "serve"
            ]));
        }
    }

    #[test]
    fn subcommands_parse_and_handover_commands_are_gone() {
        assert_eq!(
            parse(&["x", "daemon", "run", "--home", "/tmp/h"]).unwrap(),
            Command::Run {
                home: Some("/tmp/h".into())
            }
        );
        for args in [
            &["x", "daemon", "status"][..],
            &["x", "daemon", "pause"],
            &["x", "daemon", "resume"],
            &["x", "daemon", "stop"],
            &["x", "daemon", "sign-out"],
            &["x", "daemon", "install"],
            &["x", "daemon", "uninstall"],
            &[
                "x",
                "daemon",
                "enroll",
                "--server",
                "https://s.example/",
                "--owner",
                "@o:s.example",
            ],
        ] {
            assert!(parse(args).is_ok(), "{args:?}");
        }
        for args in [
            &["x", "daemon", "release"][..],
            &["x", "daemon", "enroll", "--server", "https://s.example/"],
            &["x", "daemon", "enroll", "--owner", "@o:s.example"],
            &[
                "x",
                "daemon",
                "enroll",
                "--server",
                "https://s.example/",
                "--owner",
                "@o:s.example",
                "--adopt-root",
                "/x",
            ],
            &["x", "daemon", "run", "--agent-driver", "x"],
            &["x", "daemon"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn the_launch_agent_arguments_dispatch_to_daemon_run() {
        let exe = Path::new("/Applications/Hagency.app/Contents/MacOS/hagency-desktop");
        let home = Path::new("/Users/a/Library/Application Support/Hagency/owner-daemon");
        let args = crate::service::daemon_arguments(exe, home, None);
        assert!(requested(&args));
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        assert_eq!(
            parse(&args).unwrap(),
            Command::Run {
                home: Some(home.into())
            }
        );
        #[cfg(all(feature = "test-support", feature = "resource-adoption"))]
        {
            let isolated =
                crate::service::daemon_arguments(exe, home, Some(Path::new("/tmp/isolated")));
            assert!(requested(&isolated));
            let invocation = Invocation::try_parse_from(&isolated).unwrap();
            assert_eq!(
                invocation.isolated_installation_root,
                Some(PathBuf::from("/tmp/isolated"))
            );
        }
    }

    #[test]
    fn dispatch_runs_the_command_and_reports_its_exit_status() {
        let temp = tempfile::Builder::new()
            .prefix("hc")
            .tempdir_in("/tmp")
            .unwrap();
        let home = temp.path().join("daemon").to_string_lossy().into_owned();
        // No daemon runs on this home: status answers without creating it.
        assert_eq!(
            run_args(
                ["x", "daemon", "status", "--home", &home]
                    .map(OsString::from)
                    .to_vec()
            ),
            0
        );
        assert!(!Path::new(&home).exists());
        // Sign-out needs an enrollment; the error is a failure status.
        assert_eq!(
            run_args(
                ["x", "daemon", "sign-out", "--home", &home]
                    .map(OsString::from)
                    .to_vec()
            ),
            1
        );
        // A usage error is clap's status.
        assert_eq!(
            run_args(["x", "daemon", "release"].map(OsString::from).to_vec()),
            2
        );
    }
}
