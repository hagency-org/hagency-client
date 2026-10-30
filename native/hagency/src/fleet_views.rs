//! Fleet views (task #46, TS parity): the framework catalog, the host
//! detection probe and the role-capability view — `GET /api/frameworks`
//! (backend-v2.js:13383), `GET /api/frameworks/detect` (:13530) and
//! `GET /api/capability` (:13584), each answering the TS JSON shape from the
//! native store's real state.
//!
//! THE MANIFEST TABLE is the port of `lib/frameworks/*.json` as
//! `serializeFramework` projects it (backend-v2.js:13356-13381): the five
//! adapters in registry order (claude, codex-acp, codex, hermes, octos), the
//! launch projection and the flag guard flattened from its Set/RegExp forms
//! exactly as the TS serializer flattens them — `[...exact, ...prefix].sort()`.
//! It lives here rather than behind a JSON reader because the manifest is
//! compile-time product truth: `refusedFlags` and `guardMessage` are what a
//! client lists when it refuses a flag, and a parse failure at startup beats
//! an empty guard read as "refuses nothing" at request time (the TS comment's
//! own warning).
//!
//! THE PROBE never grows a new dependency: `tokio`'s process feature is
//! test-only in this crate, so the `which`/`--version`/ACP-subcommand probes
//! run `std::process::Command` on the blocking pool with the TS's own
//! timeouts and stdin discipline (`stdio: ignore` — "a CLI that reads it
//! waits forever, which is how a health probe becomes an outage").
use crate::refusal;
use crate::resources::domain;
use hagency_core::project::Resource;
use hagency_core::qualification::{self, Tier};
use salvo::prelude::*;
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) fn router() -> Router {
    Router::new()
        .push(Router::with_path("frameworks").get(frameworks))
        .push(Router::with_path("frameworks/detect").get(detect))
        .push(Router::with_path("capability").get(capability))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or_default()
}

/// Map a store read failure the way the operator read routes do (a 503 with a
/// code), without widening `resources::failure`'s visibility beyond its module.
fn read_failed(res: &mut Response, error: hagency_store::Error) {
    use hagency_store::Error as E;
    let (status, code) = match error {
        E::Invalid(_) => (StatusCode::BAD_REQUEST, "invalid_fleet_view"),
        E::Busy => (StatusCode::SERVICE_UNAVAILABLE, "busy"),
        E::OutcomeUnknown => (StatusCode::GATEWAY_TIMEOUT, "outcome_unknown"),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "fleet_view_unavailable"),
    };
    refusal(res, status, code);
}

// ── The manifest registry (lib/frameworks, registry order) ─────────────

struct Manifest {
    id: &'static str,
    display_name: &'static str,
    transport: &'static str,
    launchable: bool,
    not_launchable_reason: Option<&'static str>,
    command: &'static str,
    default_args: &'static [&'static str],
    /// launch.acpArgs — for an ACP adapter, element 0 is the subcommand the
    /// launch path actually drives (octos: "acp"; the others ship none).
    acp_args: &'static [&'static str],
    model_flag: Option<&'static str>,
    permission_summary: Option<&'static str>,
    acp_model_flag: Option<&'static str>,
    acp_model_flag_note: Option<&'static str>,
    command_note: Option<&'static str>,
    guard_exact: &'static [&'static str],
    guard_prefix: &'static [&'static str],
    guard_message: Option<&'static str>,
}

const CLAUDE: Manifest = Manifest {
    id: "claude",
    display_name: "Claude Code",
    transport: "tmux",
    launchable: true,
    not_launchable_reason: None,
    command: "claude",
    default_args: &["--permission-mode", "auto"],
    acp_args: &[],
    model_flag: Some("--model"),
    permission_summary: Some("auto-mode"),
    acp_model_flag: None,
    acp_model_flag_note: None,
    command_note: None,
    guard_exact: &[
        "--dangerously-skip-permissions",
        "--allow-dangerously-skip-permissions",
        "--permission-mode",
    ],
    guard_prefix: &["--permission-mode="],
    guard_message: Some("Claude permission policy flag is managed by hagency: {token}"),
};

const CODEX_ACP: Manifest = Manifest {
    id: "codex-acp",
    display_name: "Codex (ACP)",
    transport: "acp",
    launchable: false,
    not_launchable_reason: Some(
        "hagency-up creates a tmux session and an ACP agent has no pane; start it with `hagency acp-up <name> <workspace> codex-acp` instead",
    ),
    command: "codex-acp",
    default_args: &[],
    acp_args: &[],
    model_flag: Some("--model"),
    permission_summary: Some(
        "codex-acp asks before each tool call and hagency declines all but its own coordination tools; hagency passes NO filesystem sandbox flag on this transport",
    ),
    acp_model_flag: None,
    acp_model_flag_note: Some(
        "codex-acp accepts --model without complaint and then ignores it — verified: the handshake succeeds either way. Declaring null so hagency refuses the flag up front rather than letting an operator believe a model was selected when it was not.",
    ),
    command_note: Some(
        "codex-acp is an adapter, not codex itself: it wraps the Codex SDK and speaks ACP on stdio. Unlike octos and hermes, whose vendors ship ACP directly, this puts a third-party process in the chain, version-coupled to both the adapter and the codex CLI underneath it. Installed from @agentclientprotocol/codex-acp; the older @zed-industries/codex-acp is deprecated in favour of it.",
    ),
    guard_exact: &[
        "--yolo",
        "--full-auto",
        "--dangerously-bypass-approvals-and-sandbox",
        "--sandbox",
        "-s",
        "--ask-for-approval",
        "-a",
    ],
    guard_prefix: &["--sandbox=", "--ask-for-approval="],
    guard_message: Some("Codex Level 2 policy flag is managed by hagency: {token}"),
};

const CODEX: Manifest = Manifest {
    id: "codex",
    display_name: "Codex",
    transport: "tmux",
    launchable: true,
    not_launchable_reason: None,
    command: "codex",
    default_args: &[
        "--sandbox",
        "workspace-write",
        "--ask-for-approval",
        "on-request",
    ],
    acp_args: &[],
    model_flag: Some("--model"),
    permission_summary: Some("level2 (workspace-write + on-request)"),
    acp_model_flag: None,
    acp_model_flag_note: None,
    command_note: None,
    guard_exact: &[
        "--yolo",
        "--full-auto",
        "--dangerously-bypass-approvals-and-sandbox",
        "--sandbox",
        "-s",
        "--ask-for-approval",
        "-a",
    ],
    guard_prefix: &["--sandbox=", "--ask-for-approval="],
    guard_message: Some("Codex Level 2 policy flag is managed by hagency: {token}"),
};

const HERMES: Manifest = Manifest {
    id: "hermes",
    display_name: "Hermes",
    transport: "acp",
    launchable: false,
    not_launchable_reason: Some(
        "hagency-up creates a tmux session and an ACP agent has no pane; start it with `hagency acp-up <name> <workspace> hermes` instead",
    ),
    command: "hermes-acp",
    default_args: &[],
    acp_args: &[],
    model_flag: Some("--model"),
    permission_summary: Some("hermes interactive approval prompts (bypass flags refused)"),
    acp_model_flag: None,
    acp_model_flag_note: Some(
        "hermes-acp takes no model flag. Passing one is FATAL: 'hermes-acp: error: unrecognized arguments: --model x' and the process dies before initialize, exactly as it did for --cwd. Model selection is hermes-side (`hermes model`).",
    ),
    command_note: Some(
        "hermes-acp is the ACP entry point (acp_adapter.entry:main), installed by the [acp] extra. It is a separate binary rather than a subcommand, unlike `octos acp`.",
    ),
    guard_exact: &["--yolo", "--accept-hooks"],
    guard_prefix: &[],
    guard_message: Some("Hermes approval policy flag is managed by hagency: {token}"),
};

const OCTOS: Manifest = Manifest {
    id: "octos",
    display_name: "Octos",
    transport: "acp",
    launchable: false,
    not_launchable_reason: Some(
        "hagency-up creates a tmux session and an ACP agent has no pane; start it with `hagency acp-up <name> <workspace> octos` instead",
    ),
    command: "octos",
    default_args: &[],
    // launch.acpArgs: ["acp", "--profile", "coding-full"] — the subcommand
    // the ACP launch path actually drives.
    acp_args: &["acp", "--profile", "coding-full"],
    model_flag: Some("--model"),
    permission_summary: Some(
        "octos sandbox as configured (hagency never passes --danger-full-access)",
    ),
    // octos.json launch.acpArgs: ["acp", "--profile", "coding-full"] — the
    // first element is the subcommand the ACP launch path actually drives.
    acp_model_flag: Some("--model"),
    acp_model_flag_note: Some(
        "Verified: `octos acp --help` lists --model <MODEL> and the handshake survives it. Declared separately from launch.modelFlag because that one describes the tmux CLI, and the two are not the same surface — hermes-acp and codex-acp take no model flag at all even though their CLIs do.",
    ),
    command_note: None,
    guard_exact: &["--danger-full-access", "--yolo"],
    guard_prefix: &["--sandbox="],
    guard_message: Some("Octos sandbox flag is managed by hagency: {token}"),
};

/// Registry order is the manifest file order (lib/frameworks/index.js:27).
static REGISTRY: [Manifest; 5] = [CLAUDE, CODEX_ACP, CODEX, HERMES, OCTOS];

/// serializeFramework (backend-v2.js:13356-13381), field for field.
fn serialize_framework(f: &Manifest) -> Value {
    // `[...exact, ...prefix].sort()` — the TS flattening, byte for byte.
    let mut refused: Vec<&str> = f.guard_exact.to_vec();
    refused.extend(f.guard_prefix.iter().copied());
    refused.sort_unstable();
    json!({
        "id": f.id,
        "displayName": f.display_name,
        "transport": f.transport,
        "launchable": f.launchable,
        "notLaunchableReason": f.not_launchable_reason,
        "command": f.command,
        "defaultArgs": f.default_args,
        "modelFlag": f.model_flag,
        "permissionSummary": f.permission_summary,
        "acpModelFlag": f.acp_model_flag,
        "acpModelFlagNote": f.acp_model_flag_note,
        "commandNote": f.command_note,
        "refusedFlags": refused,
        "guardMessage": f.guard_message,
    })
}

/// GET /api/frameworks (backend-v2.js:13383): the adapter list, projected.
#[handler]
async fn frameworks(_req: &mut Request, _depot: &mut Depot, res: &mut Response) {
    res.render(Json(Value::Array(
        REGISTRY.iter().map(serialize_framework).collect(),
    )));
}

// ── The host probe (backend-v2.js:13419-13528) ────────────────────────

/// Where a login would live, relative to home (probeFramework's CRED map).
fn credential_home(id: &str) -> Option<&'static str> {
    Some(match id {
        "claude" => ".claude",
        "codex" | "codex-acp" => ".codex",
        "octos" => ".config/octos",
        "hermes" => ".hermes",
        _ => return None,
    })
}

/// One bounded child run's outcome. TS distinguishes a timeout (`e?.killed`)
/// from a plain failure (`e?.message`) because the fix strings differ.
enum Child {
    Success(String),
    Failed { message: String },
    TimedOut,
}

/// One bounded child run with the TS's own deadline and stdin discipline:
/// never inherit stdin, kill on timeout. `None` is a spawn failure.
fn run_bounded(command: &str, args: &[&str], timeout: Duration) -> Option<Child> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(command)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = child.wait_with_output().ok()?;
                let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                if output.status.success() {
                    return Some(Child::Success(stdout));
                }
                // TS e?.message for execFile is the command line; stderr's
                // first line is the more useful port and the ACP detail path
                // prefers it too. Both truncated like the TS (.slice(0,120)).
                let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                let message = if stderr.trim().is_empty() {
                    format!("{} {}", command, args.join(" "))
                } else {
                    stderr
                };
                return Some(Child::Failed {
                    message: message
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(120)
                        .collect(),
                });
            }
            Ok(None) => {
                if started.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Some(Child::TimedOut);
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return None,
        }
    }
}

/// probeFramework, ported with its exact state ladder and fix strings.
fn probe_sync(f: &Manifest) -> Value {
    // `which` first: a missing binary is missing, not an unexplained failure.
    let on_path = run_bounded("which", &[f.command], Duration::from_secs(3))
        .is_some_and(|o| matches!(o, Child::Success(_)));
    let mut version = Value::Null;
    let mut probe_error: Option<String> = None;
    if on_path {
        match run_bounded(f.command, &["--version"], Duration::from_secs(5)) {
            Some(Child::Success(stdout)) => {
                let first = stdout
                    .trim()
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(80)
                    .collect::<String>();
                version = if first.is_empty() {
                    Value::Null
                } else {
                    json!(first)
                };
            }
            Some(Child::TimedOut) => probe_error = Some("version probe timed out".into()),
            Some(Child::Failed { message }) => {
                probe_error = Some(if message.is_empty() {
                    "version probe failed".into()
                } else {
                    message
                });
            }
            None => probe_error = Some("version probe failed".into()),
        }
    }
    // For an ACP framework, `--version` is not evidence it can start: probe
    // the subcommand the launch path actually drives (`acpArgs[0]`).
    let acp_subcommand = (f.transport == "acp").then(|| f.acp_args.first()).flatten();
    if on_path
        && probe_error.is_none()
        && let Some(sub) = acp_subcommand
    {
        match run_bounded(f.command, &[sub, "--help"], Duration::from_secs(5)) {
            Some(Child::Success(_)) => {}
            Some(Child::TimedOut) => {
                probe_error = Some(format!("`{} {sub}` probe timed out", f.command))
            }
            Some(Child::Failed { message }) => {
                probe_error = Some(format!(
                    "installed {} has no working `{sub}` subcommand, which is how hagency starts it: {}",
                    f.command,
                    message.chars().take(100).collect::<String>()
                ))
            }
            None => {
                probe_error = Some(format!(
                    "installed {} has no working `{sub}` subcommand, which is how hagency starts it",
                    f.command
                ))
            }
        }
    }
    let cred_rel = credential_home(f.id);
    let credential_present = cred_rel.map(|rel| {
        std::env::var("HOME")
            .ok()
            .is_some_and(|home| std::path::Path::new(&home).join(rel).exists())
    });
    // `launchable: false` is a different START COMMAND, not an inability.
    let state = if !on_path {
        "absent"
    } else if probe_error.is_some() {
        "unusable"
    } else if credential_present == Some(false) {
        "needs_auth"
    } else {
        "ready"
    };
    let fix = match state {
        "absent" => format!("install {} and put it on PATH", f.command),
        "needs_auth" => format!("{} login", f.command),
        "unusable" => format!("check that `{} --version` returns", f.command),
        _ => String::new(),
    };
    json!({
        "id": f.id,
        "displayName": f.display_name,
        "transport": f.transport,
        "command": f.command,
        "onPath": on_path,
        "version": version,
        "probeError": probe_error,
        // `~/.codex`, never `/Users/someone/.codex`.
        "credentialHome": cred_rel.map(|rel| format!("~/{rel}")),
        "credentialPresent": credential_present,
        "launchable": f.launchable,
        "notLaunchableReason": f.not_launchable_reason,
        "permissionSummary": f.permission_summary,
        "state": state,
        "fix": if fix.is_empty() { Value::Null } else { json!(fix) },
        "startWith": if f.transport == "acp" { "hagency acp-up" } else { "hagency up" },
    })
}

/// GET /api/frameworks/detect (backend-v2.js:13530): what THIS host can run,
/// probed — with the caveat said in the payload, not left to the client.
#[handler]
async fn detect(_req: &mut Request, _depot: &mut Depot, res: &mut Response) {
    let host = tokio::task::spawn_blocking(|| {
        match run_bounded("hostname", &[], Duration::from_secs(2)) {
            Some(Child::Success(out)) => out.trim().to_owned(),
            _ => String::new(),
        }
    })
    .await
    .unwrap_or_default();
    let mut probes = Vec::with_capacity(REGISTRY.len());
    for manifest in &REGISTRY {
        let value = tokio::task::spawn_blocking(move || probe_sync(manifest))
            .await
            .unwrap_or_else(|_| probe_sync(manifest));
        probes.push(value);
    }
    res.render(Json(json!({
        "scannedAt": now_ms(),
        "host": host,
        "frameworks": probes,
        "caveat": "credentialPresent means the credential directory exists, not that a valid session is in it",
    })));
}

// ── Role capability (backend-v2.js:13584-13725) ───────────────────────

/// TS TIER_RANK: [...CAPABILITY_TIERS].reverse() — strength order.
fn rank(tier: Tier) -> u8 {
    match tier {
        Tier::Lightweight => 0,
        Tier::Medium => 1,
        Tier::Strong => 2,
    }
}

#[derive(serde::Deserialize)]
struct RoleView {
    #[serde(default)]
    #[serde(rename = "displayName")]
    display_name: Option<String>,
}

/// The role display names and exclusions from the same embedded policy the
/// qualification module reads — one file, one truth, no second table.
#[derive(serde::Deserialize)]
struct PolicyView {
    #[serde(default)]
    roles: std::collections::BTreeMap<String, RoleView>,
    #[serde(default)]
    excluded: Vec<Value>,
}

fn policy_view() -> PolicyView {
    serde_json::from_str(include_str!("../../hagency-core/role-capacity.json"))
        .expect("the same embedded policy the qualification module validates")
}

/// GET /api/capability: which roles this deployment can fill, and why not
/// when it cannot. Read-only, decides nothing. The agent rows come from the
/// roster (one per agent, ADR-126) joined to their backing resource for the
/// resolved model; the resources map comes from the configured resources,
/// ranked exactly as `qualification::resources_for_role` picks them.
#[handler]
async fn capability(_req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let Some(store) = domain(depot, res) else {
        return;
    };
    let roster = match store.agent_roster().await {
        Ok(rows) => rows,
        Err(error) => {
            read_failed(res, error);
            return;
        }
    };
    // Every configured resource, paged through the existing bounded read,
    // kept as parallel vectors: the ROW id is the preset surface the rest of
    // the API names (`framework-presets/{id}`) and the engagement join key;
    // the config carries the model identity the tier judgement needs.
    let mut row_ids: Vec<String> = Vec::new();
    let mut flat: Vec<Resource> = Vec::new();
    // The monthly maximum in force per row (an offered resource's amendment).
    let mut ceilings: Vec<Option<u64>> = Vec::new();
    let mut after = String::new();
    loop {
        let page = match store.resource_configurations(after.clone(), 100).await {
            Ok(page) => page,
            Err(error) => {
                read_failed(res, error);
                return;
            }
        };
        let next_after = page.last().map(|row| row.id.clone());
        for row in page {
            after = row.id.clone();
            row_ids.push(row.id);
            ceilings.push(row.effective_ceiling.and_then(|c| c.tokens).map(u64::from));
            flat.push(row.config);
        }
        match next_after {
            Some(next) if next != after => after = next,
            _ => break,
        }
    }
    // `resources_for_role` answers references INTO `flat`; map one back to
    // its row id by offset (valid because every reference comes from that
    // exact slice).
    let offset = |r: &Resource| -> usize {
        (r as *const Resource as usize - flat.as_ptr() as usize) / std::mem::size_of::<Resource>()
    };
    let row_id = |r: &Resource| -> &str { row_ids[offset(r)].as_str() };
    // The resolved model and preset row id per agent: agent_detail names the
    // backing resource row; the roster alone carries only the framework.
    let mut profiles: std::collections::HashMap<String, hagency_core::qualification::ModelProfile> =
        std::collections::HashMap::new();
    let mut agent_preset: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for row in &roster {
        if profiles.contains_key(&row.name) {
            continue;
        }
        if let Ok(Some(detail)) = store.agent_detail(&row.name).await
            && let Some((_, resource)) = row_ids
                .iter()
                .zip(flat.iter())
                .find(|(id, _)| id.as_str() == detail.resource_id)
        {
            profiles.insert(row.name.clone(), resource.profile());
            agent_preset.insert(row.name.clone(), detail.resource_id.clone());
        }
    }
    let policy = policy_view();
    let mut roles = Vec::new();
    let mut resources_map = serde_json::Map::new();
    for role in qualification::roles() {
        let need = qualification::default_tier(role).unwrap_or(Tier::Strong);
        let mut able: Vec<Value> = Vec::new();
        let mut unable: Vec<Value> = Vec::new();
        for row in &roster {
            let profile = match profiles.get(&row.name) {
                Some(profile) => profile,
                None => {
                    unable.push(json!({"agent": row.name, "reason": "no-model", "model": null}));
                    continue;
                }
            };
            let (tier, family) = qualification::model(profile);
            let Some(tier) = tier else {
                unable.push(json!({
                    "agent": row.name,
                    "reason": if profile.model.is_empty() { "no-model" } else { "model-not-accepted" },
                    "model": if profile.model.is_empty() { Value::Null } else { json!(profile.model) },
                }));
                continue;
            };
            if rank(tier) < rank(need) {
                unable.push(json!({"agent": row.name, "reason": "below-tier",
                    "tier": tier, "need": need}));
                continue;
            }
            able.push(json!({
                "agent": row.name,
                "presetId": agent_preset.get(&row.name).map(|id| json!(id.as_str())).unwrap_or(Value::Null),
                "framework": row.framework,
                "model": json!(profile.model),
                "reasoning": json!(profile.reasoning),
                "tier": tier,
                "family": family.map(Value::from).unwrap_or(Value::Null),
                "overTier": rank(tier) - rank(need),
                "online": row.online,
            }));
        }
        // resourcesForRole over the configured presets, provisionable only —
        // the same canProvision filter the TS route applies.
        let ranked: Vec<&Resource> = qualification::resources_for_role(&flat, role, Some(need))
            .into_iter()
            .filter(|r| r.provisionable())
            .collect();
        let mut families: Vec<String> = able
            .iter()
            .filter_map(|a| a["family"].as_str().map(str::to_owned))
            .collect();
        families.extend(
            ranked
                .iter()
                .filter_map(|r| qualification::model(&r.profile()).1.map(str::to_owned)),
        );
        families.sort();
        families.dedup();
        let cross_family = qualification::cross_family(role);
        let represented: std::collections::BTreeSet<&str> =
            able.iter().filter_map(|a| a["presetId"].as_str()).collect();
        let provisionable = ranked
            .iter()
            .filter(|r| !represented.contains(row_id(r)))
            .count();
        let display_name = policy
            .roles
            .get(role)
            .and_then(|r| r.display_name.clone())
            .unwrap_or_else(|| role.to_owned());
        // TS: overTier counts the able rows whose tier exceeds the floor.
        let over_tier = able
            .iter()
            .filter(|a| a["overTier"].as_u64().is_some_and(|v| v > 0))
            .count();
        roles.push(json!({
            "role": role,
            "displayName": display_name,
            "defaultTier": need,
            "crossFamily": cross_family,
            "crossFamilyOk": if cross_family { families.len() >= 2 } else { true },
            "families": families,
            // FILLABLE MEANS STAFFABLE: the cross-family rule gates the
            // headline, `able` still lists every agent that clears the tier.
            "fillable": if cross_family && families.len() < 2 { 0 } else { able.len() + provisionable },
            "able": able,
            "unable": unable,
            "overTier": over_tier,
            "excluded": policy.excluded.iter().filter(|e| e["role"] == role).cloned().collect::<Vec<_>>(),
        }));
        let qualified: Vec<Value> = ranked
            .iter()
            .map(|r| {
                let (tier, family) = qualification::model(&r.profile());
                json!({
                    "presetId": row_id(r),
                    "name": r.preset_id,
                    "framework": r.framework,
                    "model": r.model,
                    "reasoning": r.reasoning,
                    "family": family.map(Value::from).unwrap_or(Value::Null),
                    "tier": tier,
                    "overTier": tier.map(|t| rank(t) - rank(need)).unwrap_or(0),
                    "ceilingTokens": ceilings[offset(r)],
                })
            })
            .collect();
        let qualified_ids: std::collections::BTreeSet<&str> =
            ranked.iter().map(|r| row_id(r)).collect();
        let unqualified: Vec<Value> = flat
            .iter()
            .filter(|r| !qualified_ids.contains(row_id(r)))
            .map(|r| {
                let tier = qualification::model(&r.profile()).0;
                json!({
                    "presetId": row_id(r),
                    "name": r.preset_id,
                    "tier": tier,
                    "reason": if !r.provisionable() { "framework-not-provisionable" }
                        else if tier.is_none() { if r.model.is_empty() { "no-model" } else { "model-not-accepted" } }
                        else if rank(tier.unwrap()) < rank(need) { "below-tier" }
                        else { "no-ceiling" },
                })
            })
            .collect();
        resources_map.insert(
            role.to_owned(),
            json!({
                "qualified": qualified,
                "selected": ranked.first().map(|r| json!(row_id(r))).unwrap_or(Value::Null),
                "unqualified": unqualified,
                // An empty `qualified` must not read as "no presets exist".
                "considered": row_ids.len(),
            }),
        );
    }
    res.render(Json(json!({
        "generatedAt": now_ms(),
        "tiers": ["strong", "medium", "lightweight"],
        "agents": roster.len(),
        "source": "native/hagency-core/role-capacity.json",
        "roles": roles,
        "resources": resources_map,
    })));
}
