//! The executable has exactly one architecture: Pasion-authorized user-owned
//! Agents. Legacy Fleet/Engagement commands and drivers are not CLI options.
use clap::{Parser, Subcommand};
use std::{net::SocketAddr, path::PathBuf};
#[derive(Parser)]
#[command(version, about = "Hagency user-owned Agent client")]
struct Cli {
    /// Match an explicitly isolated Desktop installation (development builds only).
    #[cfg(all(feature = "test-support", feature = "resource-adoption"))]
    #[arg(long = "hagency-isolated-installation-root", global = true, hide = true)]
    isolated_installation_root: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    /// Initialize the new owner-client format in an empty private directory.
    Init {
        #[arg(long)]
        state_dir: PathBuf,
    },
    /// Serve only owner login, Agent policy, provider login and execution APIs.
    Serve {
        #[arg(long)]
        state_dir: Option<PathBuf>,
        #[arg(long, default_value = "127.0.0.1:13300")]
        listen: SocketAddr,
        #[arg(long)]
        console_assets: Option<PathBuf>,
    },
    /// Start the owner console. Agents still require an explicit owner start.
    Start {
        #[arg(long)]
        state_dir: Option<PathBuf>,
        #[arg(long, default_value = "127.0.0.1:13300")]
        listen: SocketAddr,
        #[arg(long)]
        no_open: bool,
        #[arg(long)]
        console_assets: Option<PathBuf>,
    },
    /// Open the running owner's console through private local IPC.
    Open {
        #[arg(long)]
        state_dir: Option<PathBuf>,
        #[arg(long)]
        no_open: bool,
    },
    /// Install or uninstall the per-user owner-client service.
    Service {
        #[command(subcommand)]
        command: hagency::service::Command,
    },
    /// Per-user owner daemon: hosts the enrolled owner independently of any UI.
    #[cfg(unix)]
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
}
#[cfg(unix)]
#[derive(Subcommand)]
enum DaemonCommand {
    /// Run the daemon in the foreground (used by the LaunchAgent).
    Run {
        #[arg(long)]
        home: Option<PathBuf>,
    },
    /// Sign the daemon in through the browser with its own device and record
    /// the owner's one registered owner root (ADR-192).
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
    /// Explicit owner sign-out: revoke native and SDK credentials (daemon stopped).
    SignOut {
        #[arg(long)]
        home: Option<PathBuf>,
    },
    /// Register the daemon as this user's LaunchAgent / systemd user service.
    Install {
        #[arg(long)]
        home: Option<PathBuf>,
    },
    /// Remove the daemon service; its home and enrollment are kept.
    Uninstall,
}
fn absolute(path: PathBuf) -> Result<PathBuf, std::io::Error> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(result) = hagency::native_owner::run_guardian_if_requested() {
        if result.is_err() {
            std::process::exit(1);
        }
        return Ok(());
    }

    let cli = Cli::parse();
    #[cfg(all(feature = "test-support", feature = "resource-adoption"))]
    if let Some(root) = &cli.isolated_installation_root {
        if !root.is_absolute() {
            return Err("isolated installation root must be absolute".into());
        }
        hagency::native_owner::initialize_resource_activation_fixture(root)
            .map_err(|error| error.code)?;
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async_main(cli.command.unwrap_or(Command::Start {
        state_dir: None,
        listen: "127.0.0.1:13300".parse().expect("static address"),
        no_open: false,
        console_assets: None,
    })))
}
async fn async_main(command: Command) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Command::Init { state_dir } => {
            let state = hagency::owner_host::initialize(&absolute(state_dir)?)?;
            println!("Initialized owner-client state: {}", state.display());
        }
        Command::Serve {
            state_dir,
            listen,
            console_assets,
        } => serve(state_dir, listen, console_assets, false).await?,
        Command::Start {
            state_dir,
            listen,
            no_open,
            console_assets,
        } => serve(state_dir, listen, console_assets, !no_open).await?,
        Command::Open { state_dir, no_open } => {
            let state = absolute(match state_dir {
                Some(path) => path,
                None => hagency::service::default_state_dir()?,
            })?;
            let url = hagency::owner_host::request_access(&state).await?;
            println!("{url}");
            if !no_open {
                hagency::service::open_owner_link(&url);
            }
        }
        Command::Service { command } => println!("{}", hagency::service::run(command)?),
        #[cfg(unix)]
        Command::Daemon { command } => daemon(command).await?,
    }
    Ok(())
}
#[cfg(unix)]
fn daemon_home(home: Option<PathBuf>) -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(absolute(match home {
        Some(path) => path,
        None => hagency::daemon::host::default_home()?,
    })?)
}
#[cfg(unix)]
async fn daemon_call(
    home: &std::path::Path,
    command: hagency::daemon::Command,
) -> Result<hagency::daemon::Reply, Box<dyn std::error::Error>> {
    use hagency::daemon::{DaemonClient, Request, new_request_id};
    let mut client = DaemonClient::connect(home).await?;
    let hello = client
        .call(&Request::new(new_request_id(), None, hagency::daemon::Command::Hello))
        .await?
        .hello()
        .map_err(|e| e.message)?;
    let request = Request::new(new_request_id(), hello.owner_id, command);
    // An uncertain reply is retried once with the same request ID.
    match client.call(&request).await {
        Ok(reply) => Ok(reply),
        Err(hagency::daemon::IpcError::Uncertain) => {
            Ok(DaemonClient::connect(home).await?.call(&request).await?)
        }
        Err(e) => Err(e.into()),
    }
}
#[cfg(unix)]
async fn daemon(command: DaemonCommand) -> Result<(), Box<dyn std::error::Error>> {
    use hagency::daemon::{Command as C, host};
    match command {
        DaemonCommand::Run { home } => {
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
        DaemonCommand::Enroll {
            home,
            server,
            owner,
            display_name,
            cancel_on_stdin_close,
            no_open,
        } => {
            let home = daemon_home(home)?;
            let control=std::sync::Arc::new(host::EnrollmentControl::default());
            let enrollment = host::enroll_controlled(
                &home,
                &server,
                &owner,
                display_name,
                control.clone(),
                |url| async move {
                    println!("Sign in to enroll the Hagency owner daemon: {url}");
                    if !no_open {
                        hagency::service::open_owner_link(&url);
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
                let cancel_control=control.clone();
                let watcher=tokio::spawn(async move {let _=cancelled.await;cancel_control.cancel();});
                let report=enrollment.await;watcher.abort();report?
            } else { enrollment.await? };
            match report.pending {
                None => println!(
                    "Enrolled {} (owner root {}). Start the daemon with: hagency daemon run",
                    report.owner,
                    report.owner_root.display()
                ),
                Some(code) => println!(
                    "Enrolled {} but native registration is pending ({code}). The owner root may still be in use by another process.",
                    report.owner
                ),
            }
        }
        DaemonCommand::Status { home } => {
            let home = daemon_home(home)?;
            match host::probe(&home).await {
                host::Presence::NotRunning => println!("Hagency owner daemon is not running."),
                host::Presence::Unsafe => return Err("the daemon socket is not private to this user".into()),
                host::Presence::Running(_) => {
                    let reply = daemon_call(&home, C::Snapshot { period: None, usage_filter:None }).await?;
                    println!("{}", serde_json::to_string_pretty(&reply)?);
                }
            }
        }
        DaemonCommand::Pause { home } => {
            let reply = daemon_call(&daemon_home(home)?, C::PauseHosting).await?;
            println!("{}", serde_json::to_string_pretty(&reply)?);
        }
        DaemonCommand::Resume { home } => {
            let reply = daemon_call(&daemon_home(home)?, C::ResumeHosting).await?;
            println!("{}", serde_json::to_string_pretty(&reply)?);
        }
        DaemonCommand::Stop { home } => {
            let reply = daemon_call(&daemon_home(home)?, C::StopHosting).await?;
            println!("{}", serde_json::to_string_pretty(&reply)?);
        }
        DaemonCommand::SignOut { home } => {
            host::sign_out(&daemon_home(home)?).await?;
            println!("Owner signed out of the Hagency owner daemon.");
        }
        DaemonCommand::Install { home } => {
            println!("{}", hagency::service::install_daemon(&daemon_home(home)?)?)
        }
        DaemonCommand::Uninstall => println!("{}", hagency::service::uninstall_daemon()?),
    }
    Ok(())
}
async fn serve(
    state: Option<PathBuf>,
    listen: SocketAddr,
    assets: Option<PathBuf>,
    open: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let state = absolute(match state {
        Some(path) => path,
        None => hagency::service::default_state_dir()?,
    })?;
    let host = hagency::owner_host::OwnerHost::open(&state, listen, assets.as_deref())?;
    let cancel = hagency_matrix::CancellationToken::new();
    let signal = cancel.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            if let Ok(mut termination) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                tokio::select! {_=tokio::signal::ctrl_c()=>{},_=termination.recv()=>{}}
            } else {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
        signal.cancel();
    });
    let announcement = tokio::spawn(hagency::service::announce(
        host.state_directory().into(),
        listen,
        open,
    ));
    let result = host.serve(listen, &cancel).await;
    announcement.abort();
    let _ = announcement.await;
    result?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_commands_and_runtime_switches_are_not_parseable() {
        for command in [
            "setup",
            "registration",
            "side-registration",
            "provision",
            "account",
            "engagements",
            "resources",
            "alerts",
            "console-access",
            "backup",
            "restore",
            "rotate",
            "task",
            "mcp",
            "guardian",
            "intake-refuse-stale-session",
        ] {
            assert!(
                Cli::try_parse_from(["hagency", command]).is_err(),
                "legacy command accepted: {command}"
            );
        }
        for flag in [
            "--palpo-transport",
            "--agent-driver",
            "--development-driver",
            "--queue-capacity",
            "--no-local-codex",
            "--codex-home",
        ] {
            assert!(
                Cli::try_parse_from(["hagency", "serve", flag]).is_err(),
                "legacy switch accepted: {flag}"
            );
        }
        assert!(Cli::try_parse_from(["hagency"]).unwrap().command.is_none());
        assert!(matches!(
            Cli::try_parse_from(["hagency", "serve"]).unwrap().command,
            Some(Command::Serve { .. })
        ));
    }
    #[cfg(unix)]
    #[test]
    fn isolated_installation_is_only_a_development_startup_option() {
        let parsed = Cli::try_parse_from([
            "hagency", "--hagency-isolated-installation-root", "/tmp/isolated owner", "daemon", "run",
        ]);
        #[cfg(all(feature = "test-support", feature = "resource-adoption"))]
        assert_eq!(parsed.unwrap().isolated_installation_root, Some(PathBuf::from("/tmp/isolated owner")));
        #[cfg(not(all(feature = "test-support", feature = "resource-adoption")))]
        assert!(parsed.is_err());
    }
    #[cfg(unix)]
    #[test]
    fn daemon_commands_parse_and_enrollment_requires_a_server_and_owner() {
        for args in [
            &["hagency", "daemon", "run", "--home", "/tmp/h"][..],
            &["hagency", "daemon", "status"],
            &["hagency", "daemon", "pause"],
            &["hagency", "daemon", "resume"],
            &["hagency", "daemon", "stop"],
            &["hagency", "daemon", "sign-out"],
            &["hagency", "daemon", "install"],
            &["hagency", "daemon", "uninstall"],
            &["hagency", "daemon", "enroll", "--server", "https://s.example/", "--owner", "@o:s.example"],
        ] {
            assert!(
                matches!(Cli::try_parse_from(args).unwrap().command, Some(Command::Daemon { .. })),
                "{args:?}"
            );
        }
        assert!(Cli::try_parse_from(["hagency", "daemon", "enroll"]).is_err());
        assert!(Cli::try_parse_from(["hagency", "daemon", "release"]).is_err());
        assert!(Cli::try_parse_from(["hagency", "daemon", "enroll", "--server", "https://s.example/", "--adopt-root", "/x"]).is_err());
        assert!(Cli::try_parse_from(["hagency", "daemon", "run", "--agent-driver", "x"]).is_err());
    }
}
