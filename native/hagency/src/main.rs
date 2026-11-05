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
    /// Dispatched by `hagency::daemon::cli` before this parser runs.
    #[cfg(unix)]
    Daemon {
        #[command(subcommand)]
        command: hagency::daemon::cli::Command,
    },
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
    if let Some(status) = hagency::daemon::cli::run_if_requested() {
        if status != 0 {
            std::process::exit(status);
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
        Command::Daemon { command } => hagency::daemon::cli::run(command).await?,
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
            assert!(hagency::daemon::cli::requested(args), "{args:?}");
            assert!(
                matches!(Cli::try_parse_from(args).unwrap().command, Some(Command::Daemon { .. })),
                "{args:?}"
            );
        }
        assert!(Cli::try_parse_from(["hagency", "daemon", "enroll"]).is_err());
        assert!(Cli::try_parse_from(["hagency", "daemon", "release"]).is_err());
        assert!(Cli::try_parse_from(["hagency", "daemon", "enroll", "--server", "https://s.example/", "--adopt-root", "/x"]).is_err());
        assert!(Cli::try_parse_from(["hagency", "daemon", "run", "--agent-driver", "x"]).is_err());
        assert!(!hagency::daemon::cli::requested(&["hagency", "serve"]));
    }
}
