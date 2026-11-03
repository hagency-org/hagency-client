//! Owner-client start helpers and per-user service registration.
//!
//! The service runs as the user who installs it (a macOS LaunchAgent or a
//! `systemd --user` unit), so it sees that user's coding-agent sign-in and
//! `PATH`. It runs `hagency start --no-open`; agents never auto-start.
//! The console navigation URL does not grant authorization.
use std::{
    io::IsTerminal,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(clap::Subcommand)]
pub enum Command {
    /// Register and start the per-user service, then open the console.
    Install {
        #[arg(long)]
        state_dir: Option<PathBuf>,
        #[arg(long, default_value = "127.0.0.1:13300")]
        listen: SocketAddr,
        /// Do not open the console in the browser.
        #[arg(long)]
        no_open: bool,
    },
    /// Stop and remove the per-user service. The state directory is kept.
    Uninstall,
}

const LABEL: &str = "io.hagency";

/// `~/Library/Application Support/HagencyOwnedClient` (macOS) or
/// `$XDG_DATA_HOME/hagency`, else `~/.local/share/hagency` (Linux).
pub fn default_state_dir() -> Result<PathBuf, String> {
    let home = home()?;
    if cfg!(target_os = "macos") {
        return Ok(home.join("Library/Application Support/HagencyOwnedClient"));
    }
    Ok(match std::env::var_os("XDG_DATA_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("hagency"),
        _ => home.join(".local/share/hagency-owned-client"),
    })
}

/// Owner daemon home (ADR 0012): `~/Library/Application Support/HagencyOwnerDaemon`
/// on macOS, else `$XDG_STATE_HOME/hagency-owner-daemon` or
/// `~/.local/state/hagency-owner-daemon`. Separate from the owner-client state
/// directory, whose entries are a closed set.
pub fn default_daemon_dir() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("HAGENCY_OWNER_DAEMON_HOME") { return Ok(PathBuf::from(path)); }
    let home = home()?;
    if cfg!(target_os = "macos") {
        return Ok(home.join("Library/Application Support/Hagency/owner-daemon"));
    }
    Ok(match std::env::var_os("XDG_STATE_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("hagency/owner-daemon"),
        _ => home.join(".local/state/hagency/owner-daemon"),
    })
}

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_owned())
}

/// Wait until the service answers, then print its console sign-in link and,
/// when asked and this is an interactive terminal, open it in the browser.
pub async fn announce(state: PathBuf, address: SocketAddr, open: bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if crate::console::client::reachable(address).await {
            // Navigation URL grants no authority. Login must use Pasion and the
            // user's Matrix account; no operator token/ticket is ever read.
            if std::io::stdout().is_terminal()
                && let Ok(link) = crate::owner_host::request_access(&state).await
            {
                println!("Hagency owner console: {link}");
                if open {
                    open_browser(&link);
                }
                return;
            }
            println!(
                "Hagency owner client is ready. Open it with: hagency open --state-dir {}",
                state.display()
            );
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    eprintln!("Hagency owner console did not answer within 60 seconds");
}

pub fn open_owner_link(link: &str) {
    open_browser(link);
}
fn open_browser(link: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(link)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

pub fn run(command: Command) -> Result<String, String> {
    match command {
        Command::Install {
            state_dir,
            listen,
            no_open,
        } => {
            let state = match state_dir {
                Some(dir) => dir,
                None => default_state_dir()?,
            };
            let state = if state.is_absolute() {
                state
            } else {
                std::env::current_dir()
                    .map_err(|e| e.to_string())?
                    .join(state)
            };
            if !listen.ip().is_loopback() || listen.port() == 0 {
                return Err("--listen must be a nonzero loopback address".into());
            }
            crate::owner_host::initialize(&state).map_err(|e| e.to_string())?;
            let exe = std::env::current_exe()
                .and_then(|p| p.canonicalize())
                .map_err(|e| format!("this binary's path: {e}"))?;
            let path = std::env::var("PATH").unwrap_or_default();
            if cfg!(target_os = "macos") {
                install_launchd(&exe, &state, listen, &path)?;
            } else {
                install_systemd(&exe, &state, listen, &path)?;
            }
            let runtime = tokio::runtime::Handle::try_current();
            let announce = announce(state.clone(), listen, !no_open);
            match runtime {
                Ok(handle) => tokio::task::block_in_place(|| handle.block_on(announce)),
                Err(_) => tokio::runtime::Runtime::new()
                    .map_err(|e| e.to_string())?
                    .block_on(announce),
            }
            Ok(format!(
                "Hagency is installed as a service (state: {}).",
                state.display()
            ))
        }
        Command::Uninstall => {
            if cfg!(target_os = "macos") {
                uninstall_launchd()
            } else {
                uninstall_systemd()
            }
        }
    }
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn launchd_plist(exe: &Path, state: &Path, listen: SocketAddr, path: &str, log: &Path) -> String {
    let args = [
        exe.to_string_lossy().into_owned(),
        "start".into(),
        "--state-dir".into(),
        state.to_string_lossy().into_owned(),
        "--listen".into(),
        listen.to_string(),
        "--no-open".into(),
    ];
    let args: String = args
        .iter()
        .map(|a| format!("    <string>{}</string>\n", xml(a)))
        .collect();
    let log = xml(&log.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
{args}  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>{path}</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#,
        path = xml(path),
    )
}

fn launchd_path() -> Result<PathBuf, String> {
    Ok(home()?.join(format!("Library/LaunchAgents/{LABEL}.plist")))
}

fn launchctl(args: &[&str]) -> Result<std::process::Output, String> {
    std::process::Command::new("launchctl")
        .args(args)
        .output()
        .map_err(|e| format!("launchctl: {e}"))
}

fn uid() -> String {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

fn install_launchd(exe: &Path, state: &Path, listen: SocketAddr, path: &str) -> Result<(), String> {
    let logs = home()?.join("Library/Logs/Hagency");
    std::fs::create_dir_all(&logs).map_err(|e| format!("{}: {e}", logs.display()))?;
    let plist = launchd_path()?;
    if let Some(parent) = plist.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let domain = format!("gui/{}", uid());
    // Replacing an installed agent: stop the old one first, or bootstrap fails.
    let _ = launchctl(&["bootout", &format!("{domain}/{LABEL}")]);
    std::fs::write(
        &plist,
        launchd_plist(exe, state, listen, path, &logs.join("hagency.log")),
    )
    .map_err(|e| format!("{}: {e}", plist.display()))?;
    let out = launchctl(&["bootstrap", &domain, &plist.to_string_lossy()])?;
    if !out.status.success() {
        return Err(format!(
            "launchctl bootstrap failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

fn uninstall_launchd() -> Result<String, String> {
    let plist = launchd_path()?;
    let _ = launchctl(&["bootout", &format!("gui/{}/{LABEL}", uid())]);
    if plist.exists() {
        std::fs::remove_file(&plist).map_err(|e| format!("{}: {e}", plist.display()))?;
    }
    Ok("Hagency service removed; its state directory was kept.".into())
}

fn systemd_unit(exe: &Path, state: &Path, listen: SocketAddr, path: &str) -> String {
    let quote = |p: &str| format!("\"{}\"", p.replace('\\', "\\\\").replace('"', "\\\""));
    format!(
        "[Unit]\nDescription=Hagency\nAfter=network-online.target\n\n[Service]\nExecStart={} start --state-dir {} --listen {listen} --no-open\nEnvironment={}\nRestart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
        quote(&exe.to_string_lossy()),
        quote(&state.to_string_lossy()),
        quote(&format!("PATH={path}")),
    )
}

fn systemd_path() -> Result<PathBuf, String> {
    let config = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home()?.join(".config"),
    };
    Ok(config.join("systemd/user/hagency.service"))
}

fn systemctl(args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .map_err(|e| format!("systemctl --user: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "systemctl --user {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

fn install_systemd(exe: &Path, state: &Path, listen: SocketAddr, path: &str) -> Result<(), String> {
    let unit = systemd_path()?;
    if let Some(parent) = unit.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&unit, systemd_unit(exe, state, listen, path))
        .map_err(|e| format!("{}: {e}", unit.display()))?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", "hagency.service"])?;
    // `enable --now` does not restart a unit that is already running: a
    // reinstall after replacing the binary must run the new one.
    systemctl(&["restart", "hagency.service"])?;
    // A user service stops at logout unless lingering is on.
    eprintln!(
        "Note: to keep Hagency running after you log out, run `loginctl enable-linger $USER` once."
    );
    Ok(())
}

fn uninstall_systemd() -> Result<String, String> {
    let _ = systemctl(&["disable", "--now", "hagency.service"]);
    let unit = systemd_path()?;
    if unit.exists() {
        std::fs::remove_file(&unit).map_err(|e| format!("{}: {e}", unit.display()))?;
    }
    let _ = systemctl(&["daemon-reload"]);
    Ok("Hagency service removed; its state directory was kept.".into())
}

/// LaunchAgent label of the owner daemon. Distinct from the console service.
pub const DAEMON_LABEL: &str = "io.hagency.owner-daemon";

/// Per-user service definition for `hagency daemon run`. KeepAlive restarts
/// it after a crash; a clean exit (owner stop of the whole service) stays down.
pub fn daemon_launchd_plist(exe: &Path, home_dir: &Path, path: &str, log: &Path) -> String {
    daemon_launchd_plist_with_installation(exe, home_dir, path, log, None)
}

fn daemon_arguments(exe: &Path, home_dir: &Path, installation: Option<&Path>) -> Vec<String> {
    let mut args = vec![exe.to_string_lossy().into_owned()];
    if let Some(root) = installation {
        args.extend([
            "--hagency-isolated-installation-root".into(),
            root.to_string_lossy().into_owned(),
        ]);
    }
    args.extend([
        "daemon".into(),
        "run".into(),
        "--home".into(),
        home_dir.to_string_lossy().into_owned(),
    ]);
    args
}

fn daemon_launchd_plist_with_installation(exe: &Path, home_dir: &Path, path: &str, log: &Path, installation: Option<&Path>) -> String {
    let args = daemon_arguments(exe, home_dir, installation);
    let args: String = args
        .iter()
        .map(|a| format!("    <string>{}</string>\n", xml(a)))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{DAEMON_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
{args}  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>{path}</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#,
        path = xml(path),
        log = xml(&log.to_string_lossy()),
    )
}

pub fn daemon_systemd_unit(exe: &Path, home_dir: &Path, path: &str) -> String {
    daemon_systemd_unit_with_installation(exe, home_dir, path, None)
}

fn daemon_systemd_unit_with_installation(exe: &Path, home_dir: &Path, path: &str, installation: Option<&Path>) -> String {
    let quote = |p: &str| format!("\"{}\"", p.replace('\\', "\\\\").replace('"', "\\\""));
    let prefix = if let Some(root) = installation {
        format!("{} --hagency-isolated-installation-root {}", quote(&exe.to_string_lossy()), quote(&root.to_string_lossy()))
    } else {
        quote(&exe.to_string_lossy())
    };
    format!(
        "[Unit]\nDescription=Hagency owner daemon\nAfter=network-online.target\n\n[Service]\nExecStart={} daemon run --home {}\nEnvironment={}\nRestart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
        prefix,
        quote(&home_dir.to_string_lossy()),
        quote(&format!("PATH={path}")),
    )
}

/// Register and start the owner daemon for this user. Explicit owner action
/// only; nothing in the daemon installs itself.
pub fn install_daemon(home_dir: &Path) -> Result<String, String> {
    if !home_dir.is_absolute() {
        return Err("--home must be absolute".into());
    }
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| format!("this binary's path: {e}"))?;
    let path = std::env::var("PATH").unwrap_or_default();
    #[cfg(not(all(feature = "test-support", feature = "resource-adoption")))]
    let installation: Option<PathBuf> = None;
    #[cfg(all(feature = "test-support", feature = "resource-adoption"))]
    let installation = crate::native_owner::resource_activation_fixture_root();
    if cfg!(target_os = "macos") {
        let logs = home()?.join("Library/Logs/Hagency");
        std::fs::create_dir_all(&logs).map_err(|e| format!("{}: {e}", logs.display()))?;
        let plist = home()?.join(format!("Library/LaunchAgents/{DAEMON_LABEL}.plist"));
        if let Some(parent) = plist.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let domain = format!("gui/{}", uid());
        let _ = launchctl(&["bootout", &format!("{domain}/{DAEMON_LABEL}")]);
        std::fs::write(
            &plist,
            daemon_launchd_plist_with_installation(&exe, home_dir, &path, &logs.join("owner-daemon.log"), installation.as_deref()),
        )
        .map_err(|e| format!("{}: {e}", plist.display()))?;
        let out = launchctl(&["bootstrap", &domain, &plist.to_string_lossy()])?;
        if !out.status.success() {
            return Err(format!(
                "launchctl bootstrap failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
    } else {
        let config = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => home()?.join(".config"),
        };
        let unit = config.join("systemd/user/hagency-owner-daemon.service");
        if let Some(parent) = unit.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&unit, daemon_systemd_unit_with_installation(&exe, home_dir, &path, installation.as_deref()))
            .map_err(|e| format!("{}: {e}", unit.display()))?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "--now", "hagency-owner-daemon.service"])?;
        systemctl(&["restart", "hagency-owner-daemon.service"])?;
    }
    Ok(format!("Hagency owner daemon installed (home: {}).", home_dir.display()))
}

/// Remove the daemon service. Its home, enrollment and owner root are kept.
pub fn uninstall_daemon() -> Result<String, String> {
    if cfg!(target_os = "macos") {
        let plist = home()?.join(format!("Library/LaunchAgents/{DAEMON_LABEL}.plist"));
        let _ = launchctl(&["bootout", &format!("gui/{}/{DAEMON_LABEL}", uid())]);
        if plist.exists() {
            std::fs::remove_file(&plist).map_err(|e| format!("{}: {e}", plist.display()))?;
        }
    } else {
        let _ = systemctl(&["disable", "--now", "hagency-owner-daemon.service"]);
        let config = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => home()?.join(".config"),
        };
        let unit = config.join("systemd/user/hagency-owner-daemon.service");
        if unit.exists() {
            std::fs::remove_file(&unit).map_err(|e| format!("{}: {e}", unit.display()))?;
        }
        let _ = systemctl(&["daemon-reload"]);
    }
    Ok("Hagency owner daemon service removed; its home and enrollment were kept.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launchd_plist_runs_start_without_opening_and_escapes_paths() {
        let plist = launchd_plist(
            Path::new("/opt/h & co/hagency"),
            Path::new("/Users/a/Library/Application Support/HagencyOwnedClient"),
            "127.0.0.1:13300".parse().unwrap(),
            "/usr/bin:/bin",
            Path::new("/Users/a/Library/Logs/Hagency/hagency.log"),
        );
        assert!(plist.contains("<string>/opt/h &amp; co/hagency</string>"));
        assert!(plist.contains("<string>start</string>"));
        assert!(plist.contains("<string>--no-open</string>"));
        assert!(
            plist.contains(
                "<string>/Users/a/Library/Application Support/HagencyOwnedClient</string>"
            )
        );
        assert!(plist.contains("<key>PATH</key>"));
        assert!(!plist.contains("--agent-driver"));
    }

    #[test]
    fn systemd_unit_quotes_paths_and_carries_path() {
        let unit = systemd_unit(
            Path::new("/opt/hagency/hagency"),
            Path::new("/home/a/.local/share/hagency"),
            "127.0.0.1:13300".parse().unwrap(),
            "/usr/bin:/bin",
        );
        assert!(unit.contains(
            "ExecStart=\"/opt/hagency/hagency\" start --state-dir \"/home/a/.local/share/hagency\" --listen 127.0.0.1:13300 --no-open"
        ));
        assert!(unit.contains("Environment=\"PATH=/usr/bin:/bin\""));
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn daemon_service_runs_daemon_with_its_own_label_and_escaped_home() {
        let plist = daemon_launchd_plist(
            Path::new("/opt/h & co/hagency"),
            Path::new("/Users/a/Library/Application Support/HagencyOwnerDaemon"),
            "/usr/bin:/bin",
            Path::new("/Users/a/Library/Logs/Hagency/owner-daemon.log"),
        );
        assert!(plist.contains(&format!("<string>{DAEMON_LABEL}</string>")));
        assert!(!plist.contains("<string>io.hagency</string>"));
        assert!(plist.contains("<string>/opt/h &amp; co/hagency</string>"));
        assert!(plist.contains("<string>daemon</string>\n    <string>run</string>"));
        assert!(plist.contains("<string>/Users/a/Library/Application Support/HagencyOwnerDaemon</string>"));
        assert!(plist.contains("<key>SuccessfulExit</key>"));
        let unit = daemon_systemd_unit(
            Path::new("/opt/hagency/hagency"),
            Path::new("/home/a/.local/state/hagency-owner-daemon"),
            "/usr/bin",
        );
        assert!(unit.contains(
            "ExecStart=\"/opt/hagency/hagency\" daemon run --home \"/home/a/.local/state/hagency-owner-daemon\""
        ));
        let dir = default_daemon_dir().unwrap();
        assert!(dir.is_absolute());
        assert_ne!(dir, default_state_dir().unwrap());
    }

    #[test]
    fn isolated_daemon_service_preserves_the_explicit_installation_on_restart() {
        let exe = Path::new("/Applications/Hagency Hosting.app/Contents/Helpers/hagency");
        let home = Path::new("/Users/a/Library/Application Support/Hagency/owner-daemon");
        let root = Path::new("/Users/a/isolated & original");
        let args = daemon_arguments(exe, home, Some(root));
        assert_eq!(&args[1..3], &["--hagency-isolated-installation-root", "/Users/a/isolated & original"]);
        assert_eq!(&args[3..], &["daemon", "run", "--home", home.to_str().unwrap()]);
        let plist = daemon_launchd_plist_with_installation(exe, home, "/usr/bin", Path::new("/tmp/daemon.log"), Some(root));
        assert!(plist.contains("<string>--hagency-isolated-installation-root</string>\n    <string>/Users/a/isolated &amp; original</string>"));
        let unit = daemon_systemd_unit_with_installation(exe, home, "/usr/bin", Some(root));
        assert!(unit.contains("--hagency-isolated-installation-root \"/Users/a/isolated & original\" daemon run"));
        assert!(!daemon_launchd_plist(exe, home, "/usr/bin", Path::new("/tmp/daemon.log")).contains("isolated-installation"));
        assert!(!daemon_systemd_unit(exe, home, "/usr/bin").contains("isolated-installation"));
    }
}
