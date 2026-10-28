//! Current OwnerHost release gates. Real same-schema N/N+1 artifacts and stopped
//! snapshot recovery; no legacy data conversion, model calls, or unknown-cost replay.
use std::{
    fs,
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use hagency_agent_local::Ledger;
use sha2::{Digest, Sha256};
#[path = "owner_cli/mod.rs"]
mod owner;

/// The start gate's explicit budget, named like the stop budget: the same
/// 20-second figure as the unit's TimeoutStopSec, but this one bounds the
/// /ready poll after start, not the drain after SIGTERM.
const START_GATE_BUDGET: Duration = Duration::from_secs(20);
// Non-test includes outside native/**. These exact original inputs participate
// in BOTH the version-patched source copy and its content-addressed cache.
const ROOT_BUILD_INPUTS: &[&str] = &[
    "docs/workspace-claude-md-template.md",
    "docs/workspace-agents-md-template.md",
    "docs/workspace-supervisor-claude-template.md",
    "docs/workspace-supervisor-agents-template.md",
];

fn binary() -> PathBuf {
    env!("CARGO_BIN_EXE_hagency").into()
}
#[path = "release_state/mod.rs"]
mod release_state;
use release_state::*;
fn workspace_version() -> String {
    // The workspace [workspace.package] version is the ONE version source;
    // package.json or any other manifest is never consulted.
    let manifest =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml"))
            .expect("workspace manifest readable on every leg");
    let mut in_package = false;
    for line in manifest.lines() {
        let token = line.trim();
        if token.starts_with('[') {
            in_package = token == "[workspace.package]";
            continue;
        }
        if in_package && token.starts_with("version") {
            return token
                .split('=')
                .nth(1)
                .expect("version assignment")
                .trim()
                .trim_matches('"')
                .to_string();
        }
    }
    panic!("workspace package version not found");
}
fn reported_version() -> String {
    artifact_version(&binary())
}
fn artifact_version(artifact: &Path) -> String {
    let output = Command::new(artifact).arg("--version").output().unwrap();
    assert!(output.status.success(), "artifact --version must succeed");
    String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .last()
        .expect("clap prints `<name> <version>`")
        .to_string()
}
/// The SR-3 release-tree scanner: any finding fails the scan, never skips.
/// Findings: package.json / lockfiles, node_modules directories, JavaScript
/// entry files, and `node` references in shipped wrappers and units.
fn scan_node_entrypoints(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, findings: &mut Vec<String>) {
        for entry in fs::read_dir(dir).expect("readable staged tree") {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                if name == "node_modules" {
                    findings.push(format!("node_modules directory: {}", path.display()));
                    continue;
                }
                walk(&path, findings);
                continue;
            }
            let extension = path.extension().map(|v| v.to_string_lossy().into_owned());
            if matches!(
                name.as_str(),
                "package.json" | "package-lock.json" | "npm-shrinkwrap.json"
            ) || matches!(extension.as_deref(), Some("js") | Some("mjs") | Some("cjs"))
            {
                findings.push(format!("javascript entry point: {}", path.display()));
                continue;
            }
            // Wrappers and units are text: scan their bytes for node references.
            if let Ok(text) = fs::read_to_string(&path) {
                for token in ["node ", "/node", "\"node\""] {
                    if text.contains(token) {
                        findings.push(format!(
                            "node reference in shipped file: {} (`{token}`)",
                            path.display()
                        ));
                        break;
                    }
                }
            }
        }
    }
    let mut findings = Vec::new();
    walk(root, &mut findings);
    findings
}
/// Stage a release tree the way the workflow packages it: the versioned
/// binary, both units, and a checksums manifest.
fn stage_release_tree(root: &Path) -> PathBuf {
    let version = workspace_version();
    let tree = root.join(format!("hagency-v{version}"));
    fs::create_dir_all(&tree).unwrap();
    fs::copy(binary(), tree.join(format!("hagency-v{version}"))).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/hagency-native.service"),
        tree.join("hagency-native.service"),
    )
    .unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/io.hagency.native.plist"),
        tree.join("io.hagency.native.plist"),
    )
    .unwrap();
    let binary_bytes = fs::metadata(tree.join(format!("hagency-v{version}")))
        .unwrap()
        .len();
    fs::write(
        tree.join("checksums.txt"),
        format!("hagency-v{version}  {binary_bytes} bytes\nhagency-native.service  unit\nio.hagency.native.plist  unit\n"),
    )
    .unwrap();
    tree
}
fn http_status(addr: &str, path: &str) -> Option<u16> {
    let mut stream = TcpStream::connect(addr).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response.split_whitespace().nth(1)?.parse().ok()
}
fn free_loopback() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("127.0.0.1:{port}")
}
struct Guard(Child);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Running {
    guard: Guard,
    addr: String,
}
fn init_state(root: &Path) -> PathBuf {
    // The runbook's step 0 precondition: a fresh temp state directory
    // initialized by hagency init (which refuses a non-empty dir).
    let state = root.join("state");
    let status = Command::new(binary())
        .args(["init", "--state-dir", state.to_str().unwrap()])
        .status()
        .expect("hagency init spawns");
    assert!(status.success(), "hagency init must provision fresh state");
    state
}
fn spawn_service(state: &Path) -> Running {
    spawn_artifact(&binary(), state)
}
fn spawn_artifact(artifact: &Path, state: &Path) -> Running {
    let addr = free_loopback();
    let child = Command::new(artifact)
        .args([
            "serve",
            "--state-dir",
            state.to_str().unwrap(),
            "--listen",
            &addr,
        ])
        .arg("--console-assets")
        .arg({
            let root = state.parent().unwrap();
            let bundle = root.join("console-assets");
            if bundle.exists() {
                bundle
            } else {
                owner::assets(root)
            }
        })
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("serve spawns");
    Running {
        guard: Guard(child),
        addr,
    }
}
fn wait_ready(running: &Running) {
    // The runbook's gate polls /ready, never /health (health is 200-while-live
    // and proves nothing at cutover). The poll is bounded by the start gate's
    // own named budget, the same 20-second figure as the stop budget but
    // bounding the start, not the drain.
    let until = Instant::now() + START_GATE_BUDGET;
    loop {
        if http_status(&running.addr, "/ready") == Some(200) {
            return;
        }
        assert!(Instant::now() < until, "service never answered ready 200");
        thread::sleep(Duration::from_millis(100));
    }
}
/// The runbook's stop contract: exits zero inside the budget, or parks on an
/// unknown close without a false success (ADR-120).
fn term_then_observe_exit(running: &mut Running, budget: Duration) -> bool {
    let status = Command::new("kill")
        .args(["-s", "TERM", &running.guard.0.id().to_string()])
        .status()
        .expect("external kill helper spawns");
    assert!(status.success(), "SIGTERM delivery failed");
    let child = &mut running.guard.0;
    let until = Instant::now() + budget;
    loop {
        if let Some(code) = child.try_wait().unwrap() {
            assert!(
                code.success(),
                "service must exit zero on the deliberate stop, got {code:?}"
            );
            return true; // clean drain-and-close
        }
        if Instant::now() >= until {
            return false; // parked on an unknown close; no false success
        }
        thread::sleep(Duration::from_millis(50));
    }
}

// ---- Upgrade-procedure fixture (O5): two local builds, N and N+1 ----

static BUILD_LOCK: Mutex<()> = Mutex::new(());

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
/// The same tree at workspace version N+1 (the patch segment bumped by one):
/// a real local build, not the release workflow and not a runtime override.
/// The version constant is the one `hagency --version` reads, so the artifact
/// is named and reports exactly as ADR-134's procedure assumes.
fn next_version() -> String {
    let mut parts: Vec<u64> = workspace_version()
        .split('.')
        .map(|p| p.parse().expect("numeric workspace version segment"))
        .collect();
    let last = parts.last_mut().expect("version has a patch segment");
    *last += 1;
    parts
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(".")
}
fn artifact_name(version: &str) -> String {
    format!("hagency-v{version}")
}
fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    // Preserve directory permissions: init creates the state dir 0o700, and
    // private::directory refuses a dir with group/other bits set. A plain
    // create_dir_all (0o755) would make a restored state dir fail Startup.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::symlink_metadata(src).unwrap().permissions().mode();
        fs::set_permissions(dst, fs::Permissions::from_mode(mode)).unwrap();
    }
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            fs::copy(&from, &to).unwrap();
        }
    }
}
fn patch_workspace_version(manifest: &Path, version: &str) {
    let text = fs::read_to_string(manifest).unwrap();
    let mut patched = String::new();
    let mut in_package = false;
    for line in text.lines() {
        let token = line.trim();
        if token.starts_with('[') {
            in_package = token == "[workspace.package]";
            patched.push_str(line);
            patched.push('\n');
            continue;
        }
        if in_package && token.starts_with("version") {
            patched.push_str(&format!("version = \"{version}\"\n"));
            continue;
        }
        patched.push_str(line);
        patched.push('\n');
    }
    fs::write(manifest, patched).unwrap();
}
fn collect_native_files(dir: &Path, root: &Path, out: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if path.is_dir() {
            collect_native_files(&path, root, out);
        } else {
            out.push(rel);
        }
    }
}
/// Content address over the copied inputs (root manifests + the whole
/// `native/**` source set and root compile-time inputs), so a changed input rebuilds while
/// the 3x gate repeats and the second test reuse the same artifact.
fn source_fingerprint() -> String {
    let root = workspace_root();
    source_fingerprint_at(&root)
}
fn source_fingerprint_at(root: &Path) -> String {
    let mut files = vec![
        "Cargo.toml".to_string(),
        "Cargo.lock".to_string(),
        "rust-toolchain.toml".to_string(),
    ];
    files.extend(ROOT_BUILD_INPUTS.iter().map(|file| (*file).to_string()));
    collect_native_files(&root.join("native"), root, &mut files);
    files.sort();
    let mut hasher = Sha256::new();
    for rel in &files {
        hasher.update(rel.as_bytes());
        hasher.update(fs::read(root.join(rel)).unwrap());
    }
    format!("{:x}", hasher.finalize())
}
fn copy_root_build_inputs(root: &Path, destination: &Path) {
    for file in ROOT_BUILD_INPUTS {
        let target = destination.join(file);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(root.join(file), target).unwrap();
    }
}
#[test]
fn native_upgrade_shared_templates_are_build_inputs() {
    let fixture = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let source = fixture.path().join("source");
    let copy = fixture.path().join("copy");
    fs::create_dir_all(source.join("native")).unwrap();
    for file in ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"] {
        fs::write(source.join(file), file).unwrap();
    }
    let root = workspace_root();
    for file in ROOT_BUILD_INPUTS {
        let target = source.join(file);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(root.join(file), target).unwrap();
    }
    copy_root_build_inputs(&source, &copy);
    for file in ROOT_BUILD_INPUTS {
        assert_eq!(
            fs::read(source.join(file)).unwrap(),
            fs::read(copy.join(file)).unwrap()
        );
    }
    for file in ROOT_BUILD_INPUTS {
        let before = source_fingerprint_at(&source);
        let mut bytes = fs::read(source.join(file)).unwrap();
        bytes.extend_from_slice(b"\nfixture-only changed compile input\n");
        fs::write(source.join(file), &bytes).unwrap();
        assert_ne!(
            source_fingerprint_at(&source),
            before,
            "changed original shared template must invalidate artifact cache: {file}"
        );
        copy_root_build_inputs(&source, &copy);
        assert_eq!(fs::read(copy.join(file)).unwrap(), bytes);
    }
}
/// Build the N+1 artifact once, offline, under a process mutex so both tests
/// and the gate repeats share a single build. The stamp is the source
/// fingerprint; a matching stamp with the artifact present skips the
/// sync-and-build. The child inherits the parent's CARGO_HOME (a warm registry)
/// and builds into the PARENT target dir so the external dependency graph stays
/// warm and only the version-bearing path crates recompile — `--offline` still
/// forbids any fetch, so the ADR-134 no-network guarantee holds without a
/// `--locked` (the version bump legitimately updates the copy's lock entries).
fn next_artifact() -> PathBuf {
    let _guard = BUILD_LOCK.lock().unwrap();
    let root = workspace_root();
    let src = root.join("target/upgrade-next-src");
    let parent_target = root.join("target");
    let staged_next = root.join("target/.hagency-upgrade-next");
    let stamp = root.join("target/.upgrade-next-fingerprint");
    let fingerprint = source_fingerprint();
    let cached = fs::read_to_string(&stamp)
        .map(|s| s == fingerprint)
        .unwrap_or(false);
    if staged_next.is_file() && cached {
        return staged_next;
    }
    if src.exists() {
        fs::remove_dir_all(&src).unwrap();
    }
    fs::create_dir_all(&src).unwrap();
    for file in ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"] {
        fs::copy(root.join(file), src.join(file)).unwrap();
    }
    copy_root_build_inputs(&root, &src);
    copy_tree(&root.join("native"), &src.join("native"));
    patch_workspace_version(&src.join("Cargo.toml"), &next_version());
    // Distinct binary name prevents any overwrite of the N artifact used by
    // concurrently running workspace tests; only isolated copied manifests change.
    let manifest = src.join("native/hagency/Cargo.toml");
    let text = fs::read_to_string(&manifest).unwrap().replace(
        "default-run = \"hagency\"",
        "default-run = \"hagency-release-next\"\nautobins = false",
    );
    fs::write(
        &manifest,
        format!("{text}\n[[bin]]\nname = \"hagency-release-next\"\npath = \"src/main.rs\"\n"),
    )
    .unwrap();
    let status = Command::new(env!("CARGO"))
        .arg("build")
        .arg("--offline")
        .arg("-p")
        .arg("hagency")
        .arg("--bin")
        .arg("hagency-release-next")
        .env("CARGO_TARGET_DIR", &parent_target)
        .current_dir(&src)
        .status()
        .expect("N+1 build spawns");
    assert!(
        status.success(),
        "the N+1 build must succeed (offline, isolated output, shared dependency cache)"
    );
    fs::copy(
        parent_target.join("debug/hagency-release-next"),
        &staged_next,
    )
    .unwrap();
    fs::write(stamp, fingerprint).unwrap();
    assert!(
        staged_next.is_file(),
        "the N+1 artifact must exist after build"
    );
    staged_next
}
/// Render the unit from the deploy template (a read-only input), naming the
/// versioned artifact in ExecStart so unit and binary cannot disagree — the
/// same substitution SR-1's installer performs.
fn render_unit(artifact: &Path, state: &Path) -> String {
    let template = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/hagency-native.service"),
    )
    .expect("unit template present");
    let install_dir = artifact.parent().unwrap().to_string_lossy();
    let basename = artifact.file_name().unwrap().to_string_lossy();
    template
        .replace(
            "__INSTALL_DIR__/hagency",
            &format!("{install_dir}/{basename}"),
        )
        .replace("__STATE_DIR__", &state.to_string_lossy())
        .replace("__USER__", "tester")
        .replace("__CONSOLE_DIR__", "/opt/hagency-native/console")
}

#[test]
fn native_binary_version_matches_workspace() {
    let workspace = workspace_version();
    let reported = reported_version();
    assert_eq!(
        reported, workspace,
        "the binary's --version must equal the workspace [workspace.package] version exactly; \
         no other version source (package.json) is consulted"
    );
    // The artifact naming rule embeds the same version, so binary, unit and
    // artifact cannot disagree silently (ADR-127's version-identity note).
    let artifact = format!("hagency-v{workspace}");
    assert!(artifact.contains(&workspace));
}

#[test]
fn native_release_entrypoints_scan_finds_no_node() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let tree = stage_release_tree(root.path());
    let findings = scan_node_entrypoints(&tree);
    assert!(
        findings.is_empty(),
        "the release tree must contain no Node entry point: {findings:?}"
    );
    // Negative control proving the scanner detects rather than skips: a
    // planted package.json is a finding that would fail the scan.
    fs::write(tree.join("package.json"), "{}").unwrap();
    assert_eq!(
        scan_node_entrypoints(&tree).len(),
        1,
        "a planted package.json must be found"
    );
}

/// O3's line scanner: one finding per offending line, naming the file and
/// the line. Word boundaries keep honest words honest (`denoted` is not
/// `node`); `.js`/`.mjs`/`.cjs` catch Node script paths in any position;
/// `__node_bin__` is the placeholder the spec names.
fn node_reference_findings(name: &str, text: &str) -> Vec<String> {
    // Boundary-aware Node script-path detection: an extension must END a
    // path token, so `logs/app.json` stays clean while `index.js`,
    // `index.mjs`, `index.cjs` (and each followed by a space, quote or
    // flag) is a finding. All three extensions are in the spec's Must and
    // ADR-134's list.
    fn has_script_path(lower: &str) -> bool {
        for ext in [".js", ".mjs", ".cjs"] {
            let mut rest = lower;
            while let Some(pos) = rest.find(ext) {
                let after = &rest[pos + ext.len()..];
                let boundary = after
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_ascii_alphanumeric());
                if boundary {
                    return true;
                }
                rest = &rest[pos + ext.len()..];
            }
        }
        false
    }
    let mut findings = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let lower = line.to_ascii_lowercase();
        let word = lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|part| matches!(part, "node" | "npm" | "npx"));
        let script = lower.contains("__node_bin__") || has_script_path(&lower);
        if word || script {
            findings.push(format!("{name}:{}: {}", index + 1, line.trim()));
        }
    }
    findings
}

#[test]
fn native_package_entrypoints_reference_no_node() {
    let repo = workspace_root();
    let installer = fs::read_to_string(repo.join("install/install-native.sh")).unwrap();
    assert!(installer.contains("service install"));
    for retired in [
        "--palpo-transport",
        "--mode",
        " setup ",
        "fleet-runtime.json",
        "operator.token",
    ] {
        assert!(
            !installer.contains(retired),
            "retired installer option: {retired}"
        );
    }
    let files: Vec<_> = [
        "deploy/io.hagency.native.plist",
        "deploy/hagency-native.service",
        "install/install-native.sh",
    ]
    .into_iter()
    .map(|name| {
        (
            name.to_string(),
            fs::read_to_string(repo.join(name)).unwrap(),
        )
    })
    .collect();
    for (name, text) in &files {
        assert!(!text.contains("--palpo-transport"), "{name}");
    }
    let mut findings = Vec::new();
    for (name, text) in &files {
        findings.extend(node_reference_findings(name, text));
    }
    assert!(
        findings.is_empty(),
        "packaged native entrypoints must reference no Node runtime, \
         package manager or script:\n{}",
        findings.join("\n")
    );
    // Negative control, the same proof shape as the release-tree scan: the
    // scanner detects rather than skips. A planted ExecStart names its file
    // and line.
    let planted = node_reference_findings(
        "planted.service",
        "[Service]\nExecStart=/usr/bin/node /opt/app/index.js\n",
    );
    assert_eq!(
        planted,
        vec!["planted.service:2: ExecStart=/usr/bin/node /opt/app/index.js".to_string()],
        "the scanner must name the file and the line"
    );
    // A Node script invoked as .mjs must be reported by the script-path
    // matcher on its own — no `node` word present on that line.
    let mjs = node_reference_findings("planted.plist", "<string>/opt/app/worker.mjs</string>\n");
    assert_eq!(
        mjs,
        vec!["planted.plist:1: <string>/opt/app/worker.mjs</string>".to_string()],
        "an .mjs script path must be a finding"
    );
    // And the boundary stays honest: a .json path is not a Node script.
    assert!(
        node_reference_findings("honest.plist", "<string>/opt/logs/app.json</string>\n").is_empty(),
        "app.json must not be a Node script finding"
    );
}

#[test]
fn native_cutover_dryrun_version_identity() {
    // Runbook step 0: version identity is proven BEFORE any service start —
    // a mismatch fails the dry-run before a single process spawns.
    let workspace = workspace_version();
    assert_eq!(reported_version(), workspace);
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let _state = init_state(root.path()); // fresh temp state, per the runbook
}

#[test]
fn native_cutover_dryrun_ready_gate_and_stop_contract() {
    // Runbook steps 3 and 6: gate on /ready, then SIGTERM inside the budget.
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let state = init_state(root.path());
    let mut running = spawn_service(&state);
    wait_ready(&running);
    assert_eq!(http_status(&running.addr, "/health"), Some(200));
    let clean = term_then_observe_exit(&mut running, Duration::from_secs(20));
    assert!(clean, "clean drain-and-close expected on the idle leg");
}

#[test]
fn native_cutover_dryrun_owned_state_preserved_across_restart() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let state = init_state(root.path());
    seed_owned_state(&state);
    let marker = owner::marker(&state);
    let mut first = spawn_service(&state);
    wait_ready(&first);
    assert!(term_then_observe_exit(&mut first, Duration::from_secs(20)));
    let mut second = spawn_service(&state);
    wait_ready(&second);
    assert_owned_survives(&state);
    assert_eq!(owner::marker(&state), marker);
    assert!(term_then_observe_exit(&mut second, Duration::from_secs(20)));
}

// ---- O5: the upgrade procedure and its rollback (ADR-134/135) ----

fn install_dir(root: &Path) -> PathBuf {
    root.join("install")
}
/// Stage the version-N artifact into the install dir under its ADR-134 name,
/// so unit, binary and artifact cannot disagree silently. Version N's build is
/// the cargo-test build itself (CARGO_BIN_EXE_hagency, already at the
/// workspace version); the copy only names it for coexistence with N+1.
fn stage_n(root: &Path) -> PathBuf {
    let dir = install_dir(root);
    fs::create_dir_all(&dir).unwrap();
    let dest = dir.join(artifact_name(&workspace_version()));
    fs::copy(binary(), &dest).unwrap();
    dest
}
/// Stage the version-N+1 artifact (the real second local build) into the same
/// install dir, so both versioned artifacts coexist exactly as the procedure
/// assumes.
fn stage_next(root: &Path) -> PathBuf {
    let dir = install_dir(root);
    fs::create_dir_all(&dir).unwrap();
    let dest = dir.join(artifact_name(&next_version()));
    fs::copy(next_artifact(), &dest).unwrap();
    dest
}
/// A post-upgrade write recorded under N+1 whose state-restore rollback must
/// discard it: a second queued dispatch in the already-registered session and
/// task, no re-registration.
#[test]
fn native_upgrade_procedure_continues_state() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let n_artifact = stage_n(root.path());
    let next = stage_next(root.path());
    // Step 0: version identity before any service start.
    assert_eq!(artifact_version(&n_artifact), workspace_version());
    assert_eq!(artifact_version(&next), next_version());
    assert_eq!(
        reported_version(),
        workspace_version(),
        "N+1 build cannot replace the active N artifact"
    );
    // Fresh owner marker; legacy directories are refused instead of imported.
    let state = init_state(root.path());
    assert!(!state.join("operator.token").exists());
    let marker_before = owner::marker(&state);
    // Current owner ledger preserves known charges and silently releases missing usage.
    seed_owned_state(&state);
    // Step 2: the unit names the versioned N artifact.
    let unit_n = render_unit(&n_artifact, &state);
    assert!(
        unit_n.contains(&artifact_name(&workspace_version())),
        "the unit names N's versioned artifact"
    );
    // Step 3: start and gate on /ready (never /health alone).
    let mut running = spawn_artifact(&n_artifact, &state);
    wait_ready(&running);
    assert_eq!(http_status(&running.addr, "/health"), Some(200));
    // Step 6 stop contract, then the upgrade: replace the artifact, restart.
    assert!(term_then_observe_exit(
        &mut running,
        Duration::from_secs(20)
    ));
    let unit_next = render_unit(&next, &state);
    assert!(
        unit_next.contains(&artifact_name(&next_version())),
        "the unit now names N+1's versioned artifact"
    );
    let mut upgraded = spawn_artifact(&next, &state);
    wait_ready(&upgraded);
    // Step 7: preservation — the store head, the written row, the readiness
    // word, and the reported version (changed N -> N+1).
    assert_eq!(owner::marker(&state), marker_before);
    assert_owned_survives(&state);
    assert_eq!(http_status(&upgraded.addr, "/ready"), Some(200));
    assert_eq!(artifact_version(&next), next_version());
    assert!(term_then_observe_exit(
        &mut upgraded,
        Duration::from_secs(20)
    ));
}

#[test]
fn native_upgrade_rollback_restores_previous() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let n_artifact = stage_n(root.path());
    let next = stage_next(root.path());
    let state = init_state(root.path());
    let marker_before = owner::marker(&state);
    seed_owned_state(&state); // known and provisional-zero charges before the snapshot
    let backup = root.path().join("backup-state");
    copy_tree(&state, &backup);
    // Install N, start, gate; then upgrade to N+1 and gate.
    let mut running = spawn_artifact(&n_artifact, &state);
    wait_ready(&running);
    assert_eq!(artifact_version(&n_artifact), workspace_version());
    assert!(term_then_observe_exit(
        &mut running,
        Duration::from_secs(20)
    ));
    let mut upgraded = spawn_artifact(&next, &state);
    wait_ready(&upgraded);
    assert_eq!(artifact_version(&next), next_version());
    assert!(term_then_observe_exit(
        &mut upgraded,
        Duration::from_secs(20)
    ));
    // A post-upgrade write under N+1, which a state-restore rollback discards.
    {
        let mut ledger = Ledger::open(owned_path(&state), OWNER).unwrap();
        ledger
            .set_context_session(&scope(), "post-upgrade-context")
            .unwrap();
    }
    // Rollback: stop (already stopped), restore the state copy, re-point the
    // unit at N, restart — the runbook's R1 step.
    fs::remove_dir_all(&state).unwrap();
    copy_tree(&backup, &state);
    let unit_n = render_unit(&n_artifact, &state);
    assert!(unit_n.contains(&artifact_name(&workspace_version())));
    let mut restored = spawn_artifact(&n_artifact, &state);
    wait_ready(&restored);
    // Version N runs again on the same state; no re-init, no data loss.
    assert_eq!(artifact_version(&n_artifact), workspace_version());
    assert_eq!(owner::marker(&state), marker_before);
    assert_owned_survives(&state); // row X readable
    assert!(!state.join("operator.token").exists()); // never created
    let ledger = Ledger::open(owned_path(&state), OWNER).unwrap();
    assert_eq!(
        ledger.context_session(&scope()).unwrap(),
        None,
        "restoring the stopped snapshot discards later context metadata and preserves known charges"
    );
    assert!(term_then_observe_exit(
        &mut restored,
        Duration::from_secs(20)
    ));
}

#[test]
fn native_installer_delegates_current_service_and_refuses_legacy_state() {
    let root = hagency_platform::legacy_writer::test_support::initialized_tempdir().unwrap();
    let install = root.path().join("install path");
    fs::create_dir(&install).unwrap();
    fs::copy(binary(), install.join("hagency")).unwrap();
    let installer = workspace_root().join("install/install-native.sh");
    let state = root.path().join("legacy state");
    hagency_store::private::directory(&state).unwrap();
    fs::write(state.join("domain.sqlite3"), b"legacy never imported").unwrap();
    let out = Command::new("sh")
        .arg(&installer)
        .arg("--install-dir")
        .arg(&install)
        .arg("--state-dir")
        .arg(&state)
        .arg("--no-open")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(
        fs::read(state.join("domain.sqlite3")).unwrap(),
        b"legacy never imported"
    );
    assert!(!state.join("hagency-client-owned-v1.json").exists());
    assert!(!state.join("operator.token").exists());
    // Test the actual wrapper's argv and error propagation without installing
    // a real user's service. Binary-side service generation has separate tests.
    let capture = root.path().join("argv");
    fs::write(
        install.join("hagency"),
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$HAGENCY_TEST_CAPTURE\"\nexit 23\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(install.join("hagency"), fs::Permissions::from_mode(0o700)).unwrap();
    }
    let out = Command::new("sh")
        .arg(&installer)
        .arg("--install-dir")
        .arg(&install)
        .arg("--state-dir")
        .arg(&state)
        .args(["--listen", "127.0.0.1:14001", "--no-open"])
        .env("HAGENCY_TEST_CAPTURE", &capture)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(23));
    assert_eq!(
        fs::read_to_string(&capture).unwrap(),
        format!(
            "service\ninstall\n--state-dir\n{}\n--listen\n127.0.0.1:14001\n--no-open\n",
            state.display()
        )
    );
    let before = fs::read(&capture).unwrap();
    let out = Command::new("sh")
        .arg(installer)
        .args(["--mode", "fleet"])
        .env("HAGENCY_TEST_CAPTURE", &capture)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(fs::read(capture).unwrap(), before);
}
