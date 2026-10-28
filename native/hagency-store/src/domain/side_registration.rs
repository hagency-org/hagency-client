//! Project-side appservice registration issuing (task #13): the TS
//! `lib/appservice-receiver.js` `generateRegistration` +
//! `renderRegistrationYaml` pair and the `POST
//! /api/project-sides/:id/registration-file` behaviour behind them
//! (`backend-v2.js:10031-10168`), as one store-owned method — the CLI
//! (`hagency side-registration`) and the console route
//! (`console/side_registration.rs`) are both thin callers of it.
//!
//! TOKENS ARE RANDOM, NEVER DERIVED (the TS comment above
//! `generateRegistration`, ADR-014 decision 3): 32 random bytes per token
//! as hex, generated once, stored here. Nothing outside this module and
//! the private state directory ever sees them — `IssueSideRegistration`
//! returns fingerprints (sha256 hex, first 8), never a token.
//!
//! THE SERVICE READS THE STORED TOKEN WITHOUT HAND-PLACING FILES: the
//! acceptance requirement. When a credential becomes LIVE, its as-token is
//! persisted at `<state>/matrix.appservice_token` — exactly the path the
//! appservice profile reads at startup (`bootstrap/config.rs:646`,
//! `read(&state.join("matrix.appservice_token"), 4096)`) through the
//! store's private-file policy — and the full YAML at
//! `<state>/registrations/<side>.yaml` (0700 directory, 0600 file, like
//! TS's `$HAGENCY_RUNTIME_DIR/registrations`) for the operator's install
//! step. A STAGED reissue writes only the YAML: the live credential (and
//! so the token the service authenticates with) keeps working until a
//! verify proves the homeserver accepts the new one.
use crate::{Error, private};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

/// TS default `senderLocalpart` (`appservice-receiver.js:73`).
const DEFAULT_SENDER_LOCALPART: &str = "hagency";
/// TS default user namespace: `@` + `MATRIX_AGENT_PREFIX` (default `ac_`)
/// + `.*` (`appservice-receiver.js:74`, `backend-v2.js:1887,10083`).
const DEFAULT_USER_NAMESPACE: &str = "@ac_.*";

/// The caller's request: the TS route body plus which side to issue for.
#[derive(Debug, Clone)]
pub struct IssueSideRegistrationRequest {
    /// The project side — its server name, the ADR-016 identity: one
    /// homeserver, one credential.
    pub side: String,
    /// TS body `url`: the address this side's homeserver reaches Hagency
    /// at. AN INPUT, never derived (`backend-v2.js:9993-9999`).
    pub url: String,
    /// TS body `registration_id`, defaulting to `hagency-<server name>`.
    pub registration_id: Option<String>,
    /// TS body `sender_localpart`, default `hagency`.
    pub sender_localpart: Option<String>,
    /// TS body `user_namespace`, default `@ac_.*`.
    pub user_namespace: Option<String>,
    /// TS body `exclusive !== false` — true unless explicitly false.
    pub exclusive: Option<bool>,
}

/// The operator-facing result: the TS response body minus `ok`, which the
/// callers render. NEVER carries `as_token`/`hs_token` — fingerprints
/// only, exactly the TS keys (`backend-v2.js:10110-10168`):
/// camelCase, like every field the TS route returns.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueSideRegistration {
    pub staged: bool,
    /// The YAML file's path on this host.
    pub path: String,
    pub mode: &'static str,
    pub registration_id: String,
    pub sender_localpart: String,
    pub representative: String,
    pub namespace: String,
    pub url: String,
    pub as_token_fingerprint: String,
    pub hs_token_fingerprint: String,
    /// The TS `nextSteps` array, verbatim — rendered by both callers.
    pub next_steps: [&'static str; 5],
    /// TS renders `stagedNote` only when the issue was staged, verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub staged_note: Option<String>,
    /// TS renders `replacedNote` only when the live credential was KNOWN
    /// broken, verbatim (`backend-v2.js:10144-10147`) — "not staged" has
    /// two causes (nothing was there, or what was there was broken) and
    /// the operator should not have to infer which.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaced_note: Option<String>,
}

/// The TS `stagedNote` text, quoted from `backend-v2.js:10130-10134`.
pub const STAGED_NOTE: &str = "The credential this side is USING has not changed. This new one is held until you \
     install it and verification proves the homeserver accepts it, so nothing breaks in the \
     meantime — and if you generated it by mistake, ignore the file and nothing happens.";

/// The side's access verdict — the TS `accessState` values this rule reads.
/// TS only ever compares against two of them (`backend-v2.js:10060-10061`,
/// `liveIsBroken = accessState === 'rejected' || 'blocked'`), so the pair
/// that matters is spelled out here rather than left as bare strings.
// Only `Unverified` is observed natively so far (see `side_access_verdict`);
// the other verdicts are exercised by the staging-rule tests.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideAccessVerdict {
    /// A verify proved the homeserver accepts the live credential.
    Accepted,
    /// Nobody has asked yet. NOT a failure: TS protects this state
    /// (`backend-v2.js:10057-10059` — "It means nobody has asked yet, not
    /// that it fails").
    Unverified,
    /// A verify was refused; there is nothing working to protect.
    Rejected,
    /// A verify was blocked; there is nothing working to protect.
    Blocked,
}

impl SideAccessVerdict {
    /// The TS `liveIsBroken`: the two verdicts that mean the live
    /// credential is known NOT to work, so a reissue replaces it in place
    /// instead of staging behind it.
    pub fn is_broken(self) -> bool {
        matches!(self, Self::Rejected | Self::Blocked)
    }
    /// The word TS interpolates into `replacedNote`.
    fn word(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Unverified => "unverified",
            Self::Rejected => "rejected",
            Self::Blocked => "blocked",
        }
    }
}

/// TS's staging predicate, isolated from the issue path so EVERY branch is
/// testable — including the one native cannot reach yet (see
/// `side_access_verdict`). `staging = Boolean(hasCredential) && !liveIsBroken`
/// (`backend-v2.js:10061`).
fn stages(has_credential: bool, verdict: SideAccessVerdict) -> bool {
    has_credential && !verdict.is_broken()
}

/// The TS `replacedNote`, built from the verdict that made the live credential
/// not worth protecting (`backend-v2.js:10144-10147`), verbatim.
fn replaced_note(verdict: SideAccessVerdict) -> String {
    format!(
        "The previous credential was {} and has been replaced rather than held back \
         — there was nothing working to protect.",
        verdict.word()
    )
}

/// The access verdict for the side's LIVE credential.
///
/// NATIVE RECORDS NO VERDICT TODAY, so this answers `Unverified` for every
/// input — and that is the honest answer, not a placeholder: TS's
/// `unverified` is exactly "nobody has asked yet" (`backend-v2.js:10057-10059`),
/// which is the state a deployment that has never run a verify is in.
/// `console/project_sides.rs:33-46` names `access_state` among the columns it
/// has no source for, and the `verify` route that would write a verdict is
/// the parity audit's separate MISSING item (row 37,
/// `docs/parity/console-2026-09-24.md:66`).
///
/// So the TS rule above is already CORRECT today (it stages, which is what
/// TS does for `unverified`), and this function is the single place a verdict
/// lands when row 37 arrives. The `rejected`/`blocked` arm of `stages` is
/// therefore exercised by the unit test rather than by an end-to-end path —
/// and when a verdict becomes readable, the branch below is the only change.
fn side_access_verdict(
    _tx: &rusqlite::Transaction,
    _fleet: &str,
) -> Result<SideAccessVerdict, Error> {
    Ok(SideAccessVerdict::Unverified)
}

/// The TS `nextSteps` array, verbatim from `backend-v2.js:10143-10165` —
/// including the Palpo-verified TOML trap and the power-level warning.
pub const NEXT_STEPS: [&str; 5] = [
    "Put this file where your homeserver reads appservice registrations. The key differs by software: \
     Synapse takes `app_service_config_files` (a list of FILES); Palpo takes \
     `appservice_registration_dir` (a DIRECTORY, so the file goes inside it).",
    "In a TOML config the key must be TOP-LEVEL, above every [section]. Verified on Palpo: placed \
     after a section header it becomes `<that section>.appservice_registration_dir` and is silently \
     ignored — everything then fails as though the token were wrong.",
    "Restart the homeserver once. Registrations load at startup only, so nothing happens until it \
     does.",
    "Replacing tokens later needs more than replacing this file: Palpo persists registrations in its \
     database keyed by id, and a restart will not update an existing row.",
    "The representative above arrives with users_default power. A default Matrix room requires power \
     50 to invite, so either grant it that or invite each agent yourself — the approval response \
     names which agent it assigned.",
];

/// The credential the appservice transport reads. Deliberately NOT
/// `Serialize`: this type is consumed, never projected — the console
/// response is built from `IssueSideRegistration`, which cannot carry a
/// token.
#[derive(Debug, Clone, Deserialize)]
pub struct SideCredential {
    pub kind: String,
    #[serde(rename = "asToken")]
    pub as_token: String,
    #[serde(rename = "hsToken")]
    pub hs_token: String,
    pub namespace: String,
    #[serde(rename = "senderLocalpart")]
    pub sender_localpart: String,
    #[serde(default)]
    pub url: Option<String>,
}

/// The generated registration itself — private to this module; carries the
/// tokens because SOMETHING must write them to disk.
#[derive(Debug)]
struct GeneratedRegistration {
    id: String,
    url: String,
    as_token: String,
    hs_token: String,
    sender_localpart: String,
    namespace: String,
    exclusive: bool,
}

impl IssueSideRegistrationRequest {
    /// Generate the existing registration format without touching legacy files
    /// or global side records. The caller owns private credential custody.
    pub fn private_web_registration(&self) -> Result<(super::Credential, String), Error> {
        let generated = GeneratedRegistration::new(self, &self.side)?;
        let value: serde_json::Value = serde_json::from_str(&credential_row(&generated))?;
        let credential = super::Credential::from_web_configuration(&value)?;
        // JSON string quoting is also valid YAML and preserves regex escapes.
        let quote = |s: &str| serde_json::to_string(s).expect("string serialization");
        let yaml = format!("# Hagency appservice registration. Keep this file private.\nid: {}\nurl: {}\nas_token: {}\nhs_token: {}\nsender_localpart: {}\nrate_limited: false\nnamespaces:\n  users:\n    - exclusive: {}\n      regex: {}\n  aliases: []\n  rooms: []\n", quote(&generated.id), quote(&generated.url), quote(&generated.as_token), quote(&generated.hs_token), quote(&generated.sender_localpart), generated.exclusive, quote(&generated.namespace));
        Ok((credential, yaml))
    }
}

fn random_token_hex() -> Result<String, Error> {
    // TS: randomBytes(32).toString('hex') — 32 bytes, 64 hex characters.
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| Error::Unavailable)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn fingerprint(token: &str) -> String {
    // TS: createHash('sha256').update(token).digest('hex').slice(0, 8) —
    // enough to say "these differ", useless to authenticate with.
    let digest = Sha256::digest(token.as_bytes());
    digest[..4].iter().map(|b| format!("{b:02x}")).collect()
}

impl GeneratedRegistration {
    fn new(request: &IssueSideRegistrationRequest, server_name: &str) -> Result<Self, Error> {
        // TS trims the body url once, then strips trailing slashes inside
        // generateRegistration (`url.replace(/\/+$/, '')`).
        let url = request.url.trim().trim_end_matches('/').to_owned();
        if url.is_empty() {
            return Err(hagency_core::InvalidInput("url is required").into());
        }
        if url.len() > 2048 {
            return Err(hagency_core::InvalidInput("url is too long").into());
        }
        let id = match &request.registration_id {
            Some(id) => {
                if id.is_empty() || id.len() > 256 {
                    return Err(hagency_core::InvalidInput("invalid registration id").into());
                }
                id.clone()
            }
            None => format!("hagency-{server_name}"),
        };
        let sender_localpart = request
            .sender_localpart
            .as_deref()
            .unwrap_or(DEFAULT_SENDER_LOCALPART)
            .trim()
            .to_lowercase();
        if sender_localpart.is_empty() || sender_localpart.len() > 255 {
            return Err(hagency_core::InvalidInput("invalid sender localpart").into());
        }
        let namespace = request
            .user_namespace
            .as_deref()
            .unwrap_or(DEFAULT_USER_NAMESPACE)
            .trim()
            .to_owned();
        if namespace.is_empty() || namespace.len() > 255 {
            return Err(hagency_core::InvalidInput("invalid user namespace").into());
        }
        Ok(Self {
            id,
            url,
            as_token: random_token_hex()?,
            hs_token: random_token_hex()?,
            sender_localpart,
            namespace,
            exclusive: request.exclusive.unwrap_or(true),
        })
    }

    /// The homeserver registration YAML, field-for-field with TS
    /// `renderRegistrationYaml` (`appservice-receiver.js:111-136`): same
    /// comments, same key order, tokens and localpart bare (hex and a
    /// lowercase identifier — TS renders them unquoted), the url and the
    /// regex double-quoted, `rate_limited: false`, empty aliases as
    /// `aliases: []`, `rooms: []`, one trailing newline.
    fn yaml(&self) -> String {
        let exclusive = if self.exclusive { "true" } else { "false" };
        let lines = [
            "# Hagency appservice registration. Generated — do not hand-edit the tokens."
                .to_owned(),
            "# Install on the project side's homeserver and restart it: registrations load once."
                .to_owned(),
            format!("id: {}", self.id),
            format!("url: \"{}\"", self.url),
            format!("as_token: {}", self.as_token),
            format!("hs_token: {}", self.hs_token),
            format!("sender_localpart: {}", self.sender_localpart),
            "rate_limited: false".into(),
            "namespaces:".into(),
            "  users:".into(),
            format!("    - exclusive: {exclusive}"),
            format!("      regex: \"{}\"", self.namespace),
            // TS renders alias entries when an alias regex was passed and
            // `aliases: []` when not; the issuer passes none, so the empty
            // form is the only form here.
            "  aliases: []".into(),
            "  rooms: []".into(),
        ];
        format!("{}\n", lines.join("\n"))
    }
}

/// TS's side filename sanitizer (`backend-v2.js:10091`):
/// `side.id.replace(/[^\w.-]/g, '_')`.
fn side_file_name(side: &str) -> String {
    side.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-' {
                b as char
            } else {
                '_'
            }
        })
        .collect()
}

fn registration_yaml_path(state: &Path, side: &str) -> PathBuf {
    state
        .join("registrations")
        .join(format!("{}.yaml", side_file_name(side)))
}

/// The credential row's key spelling matches the TS store document
/// (`project-side-store.js:153-171`): camelCase tokens, `namespace`,
/// `senderLocalpart`, plus `url` (remembered so a later reissue can reuse
/// the address this one was built for) and the registration id.
fn credential_row(generated: &GeneratedRegistration) -> String {
    serde_json::json!({
        "kind": "appservice",
        "asToken": generated.as_token,
        "hsToken": generated.hs_token,
        "namespace": generated.namespace,
        "senderLocalpart": generated.sender_localpart,
        "url": generated.url,
        "registrationId": generated.id,
    })
    .to_string()
}

fn state_directory(db: &rusqlite::Connection) -> Result<PathBuf, Error> {
    // The repository owns `<state>/domain.sqlite3`, so its parent IS the
    // private state directory — the same root `bootstrap/config.rs` reads
    // `matrix.appservice_token` from. Derived, never stored twice.
    let database = PathBuf::from(db.path().ok_or(Error::State)?);
    database.parent().map(Path::to_path_buf).ok_or(Error::State)
}

/// `writeFileSync` semantics under the private-file policy: a NEW file is
/// created 0600-at-open (`private::write_new`); an EXISTING one is
/// truncated in place through the already-private handle, so a reissue
/// replaces the YAML without a moment where the path is absent or
/// world-readable.
fn write_private(path: &Path, value: &[u8]) -> Result<(), Error> {
    match private::open(path, false) {
        Ok(mut file) => {
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            file.write_all(value)?;
            file.sync_all()?;
            Ok(())
        }
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            private::write_new(path, value)
        }
        Err(error) => Err(error),
    }
}

/// 0700 at creation, like TS's `mkdirSync(dir, {recursive, mode: 0o700})`;
/// an existing directory is validated rather than trusted.
fn ensure_registrations_dir(state: &Path) -> Result<(), Error> {
    match private::create_directory_new(&state.join("registrations")) {
        Ok(()) => Ok(()),
        // The store's own private policy already ran on first creation;
        // the writes below re-validate through private::open regardless.
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            private::directory(&state.join("registrations"))
        }
        Err(error) => Err(error),
    }
}

impl crate::DomainRepository {
    /// Issue an appservice registration for one project side: generate
    /// random tokens, persist them (live when no credential exists,
    /// STAGED over a live one — TS `setCredential(..., {stage})`,
    /// `project-side-store.js:487-532`), write the YAML (always) and the
    /// service's as-token file (only when the credential went live), and
    /// return the fingerprints the operator sees. The store rows commit
    /// after the files are on disk, so a reported issue always has its
    /// artefacts.
    pub fn issue_side_registration(
        &mut self,
        request: &IssueSideRegistrationRequest,
        now_ms: u64,
    ) -> Result<IssueSideRegistration, Error> {
        let side = request.side.trim().to_lowercase();
        if side.is_empty() {
            return Err(hagency_core::InvalidInput("side is required").into());
        }
        // Computed before the transaction opens its borrow: the private
        // state directory is fixed for the repository's lifetime.
        let state = state_directory(&self.db)?;
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        // ADR-016's server-name identity: the side IS the registration's
        // canonical `serverName` — the same read `project_sides()` serves
        // as the wire id — never the `hf_` fleet id an operator never
        // sees. Case-folded both sides, the way TS `serverName()` folds.
        let fleet: Option<(String, String)> = tx
            .query_row(
                "SELECT fleet_id,json_extract(config,'$.serverName') FROM registrations \
                 WHERE lower(json_extract(config,'$.serverName'))=?1",
                [&side],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((fleet, server_name)) = fleet else {
            // TS: 404 {error:'project side not found'} — NotFound, never a
            // silent empty state.
            return Err(Error::NotFound);
        };
        let generated = GeneratedRegistration::new(request, &server_name)?;

        // STAGING, TS's rule (`backend-v2.js:10060-10061`): stage only when
        // a credential exists AND the live one is not known broken. A
        // staged row never disturbs the live one; promotion on verify is
        // owed separately. See `side_access_verdict` for why the verdict
        // reads `Unverified` today and why that is the honest answer.
        let has_credential: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM side_registrations WHERE fleet_id=?1 \
             AND credential IS NOT NULL)",
            [&fleet],
            |r| r.get(0),
        )?;
        let verdict = side_access_verdict(&tx, &fleet)?;
        let staged = stages(has_credential, verdict);
        if staged {
            tx.execute(
                "INSERT INTO side_registrations(fleet_id,pending,pending_at) VALUES(?1,?2,?3) \
                 ON CONFLICT(fleet_id) DO UPDATE SET \
                 pending=excluded.pending,pending_at=excluded.pending_at",
                rusqlite::params![fleet, credential_row(&generated), now_ms as i64],
            )?;
        } else {
            tx.execute(
                "INSERT INTO side_registrations(fleet_id,credential,issued_at) VALUES(?1,?2,?3) \
                 ON CONFLICT(fleet_id) DO UPDATE SET \
                 credential=excluded.credential,issued_at=excluded.issued_at, \
                 pending=NULL,pending_at=NULL",
                rusqlite::params![fleet, credential_row(&generated), now_ms as i64],
            )?;
        }
        let yaml_path = registration_yaml_path(&state, &side);
        let token_path = state.join("matrix.appservice_token");
        ensure_registrations_dir(&state)?;
        write_private(&yaml_path, generated.yaml().as_bytes())?;
        if !staged {
            write_private(&token_path, generated.as_token.as_bytes())?;
        }
        tx.commit()?;

        Ok(IssueSideRegistration {
            staged,
            path: yaml_path.display().to_string(),
            mode: "0600",
            registration_id: generated.id,
            sender_localpart: generated.sender_localpart.clone(),
            representative: format!("@{}:{server_name}", generated.sender_localpart),
            namespace: generated.namespace.clone(),
            url: generated.url.clone(),
            as_token_fingerprint: fingerprint(&generated.as_token),
            hs_token_fingerprint: fingerprint(&generated.hs_token),
            next_steps: NEXT_STEPS,
            staged_note: staged.then(|| STAGED_NOTE.to_owned()),
            replaced_note: verdict.is_broken().then(|| replaced_note(verdict)),
        })
    }

    /// The credential the appservice transport reads: the LIVE one, never
    /// a staged spare (TS `credentialFor`, `project-side-store.js:536`).
    /// Named so every future caller is grep-able.
    pub fn side_credential_for_transport(
        &self,
        side: &str,
    ) -> Result<Option<SideCredential>, Error> {
        let value: Option<String> = self
            .db
            .query_row(
                "SELECT credential FROM side_registrations s JOIN registrations r \
                 ON r.fleet_id=s.fleet_id \
                 WHERE lower(json_extract(r.config,'$.serverName'))=?1 AND s.credential IS NOT NULL",
                [side.trim().to_lowercase().as_str()],
                |r| r.get(0),
            )
            .optional()?;
        let value = match value {
            Some(value) => value,
            None => return Ok(None),
        };
        Ok(Some(serde_json::from_str(&value)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(url: &str) -> IssueSideRegistrationRequest {
        IssueSideRegistrationRequest {
            side: "example.test".into(),
            url: url.into(),
            registration_id: None,
            sender_localpart: None,
            user_namespace: None,
            exclusive: None,
        }
    }

    fn generated(url: &str, namespace: &str, exclusive: bool) -> GeneratedRegistration {
        GeneratedRegistration {
            id: "hagency-example.test".into(),
            url: url.into(),
            as_token: "a".repeat(64),
            hs_token: "b".repeat(64),
            sender_localpart: "hagency".into(),
            namespace: namespace.into(),
            exclusive,
        }
    }

    /// The TS-visible outcome, asserted literally: the exact bytes
    /// `renderRegistrationYaml` emits for the same input
    /// (`appservice-receiver.js:111-136`).
    #[test]
    fn yaml_matches_the_ts_render_field_for_field() {
        let yaml = generated("http://127.0.0.1:13443", "@ac_.*", true).yaml();
        let expected = "# Hagency appservice registration. Generated — do not hand-edit the tokens.\n\
             # Install on the project side's homeserver and restart it: registrations load once.\n\
             id: hagency-example.test\n\
             url: \"http://127.0.0.1:13443\"\n\
             as_token: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n\
             hs_token: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n\
             sender_localpart: hagency\n\
             rate_limited: false\n\
             namespaces:\n  users:\n    - exclusive: true\n      regex: \"@ac_.*\"\n  aliases: []\n  rooms: []\n";
        assert_eq!(yaml, expected);
    }

    #[test]
    fn yaml_non_exclusive_namespace_renders_false() {
        let yaml = generated("https://bridge.example", "@bot_.*", false).yaml();
        assert!(yaml.contains("    - exclusive: false\n      regex: \"@bot_.*\""));
    }

    /// TS defaults: `senderLocalpart` lowercased, `url` trailing slashes
    /// stripped, `exclusive !== false`.
    #[test]
    fn defaults_follow_ts() {
        let g = GeneratedRegistration::new(
            &IssueSideRegistrationRequest {
                side: "example.test".into(),
                url: "http://127.0.0.1:13443///".into(),
                registration_id: None,
                sender_localpart: Some("HAGENCY".into()),
                user_namespace: None,
                exclusive: Some(false),
            },
            "example.test",
        )
        .unwrap();
        assert_eq!(g.url, "http://127.0.0.1:13443");
        assert_eq!(g.sender_localpart, "hagency");
        assert!(!g.exclusive);
        let defaulted = GeneratedRegistration::new(&request("https://x"), "example.test").unwrap();
        assert_eq!(defaulted.id, "hagency-example.test");
        assert_eq!(defaulted.sender_localpart, "hagency");
        assert_eq!(defaulted.namespace, "@ac_.*");
        assert!(defaulted.exclusive);
    }

    /// TS: the 400 `url is required` refusal.
    #[test]
    fn empty_url_is_refused_like_the_ts_400() {
        let error = GeneratedRegistration::new(&request("   "), "example.test").unwrap_err();
        assert!(matches!(error, Error::Invalid(_)));
    }

    /// The TS staging rule, both branches (`backend-v2.js:10060-10061`):
    /// stage only when a credential exists AND the live one is not known
    /// broken. TS's own words for why `unverified` stages rather than
    /// replaces (`:10057-10059`): "It means nobody has asked yet, not that
    /// it fails".
    #[test]
    fn staging_rule_matches_ts_for_every_verdict() {
        // No credential: goes live, whatever the verdict says.
        for verdict in [
            SideAccessVerdict::Accepted,
            SideAccessVerdict::Unverified,
            SideAccessVerdict::Rejected,
            SideAccessVerdict::Blocked,
        ] {
            assert!(
                !stages(false, verdict),
                "a first credential always goes live ({verdict:?})"
            );
        }
        // A working-or-unexamined credential is protected: stage.
        assert!(stages(true, SideAccessVerdict::Accepted));
        assert!(
            stages(true, SideAccessVerdict::Unverified),
            "unverified is 'nobody has asked yet' — TS stages over it"
        );
        // A KNOWN-broken credential is replaced in place: `liveIsBroken`.
        assert!(!stages(true, SideAccessVerdict::Rejected));
        assert!(!stages(true, SideAccessVerdict::Blocked));
        assert!(SideAccessVerdict::Rejected.is_broken());
        assert!(SideAccessVerdict::Blocked.is_broken());
        assert!(!SideAccessVerdict::Unverified.is_broken());
    }

    /// The TS `replacedNote`, verbatim (`backend-v2.js:10144-10147`),
    /// naming the verdict that made the old credential not worth keeping.
    #[test]
    fn replaced_note_matches_ts_word_for_word() {
        assert_eq!(
            replaced_note(SideAccessVerdict::Rejected),
            "The previous credential was rejected and has been replaced rather than held back \
             — there was nothing working to protect."
        );
        assert_eq!(
            replaced_note(SideAccessVerdict::Blocked),
            "The previous credential was blocked and has been replaced rather than held back \
             — there was nothing working to protect."
        );
    }

    #[test]
    fn side_file_name_sanitizes_like_the_ts_regex() {
        assert_eq!(side_file_name("example.test"), "example.test");
        assert_eq!(side_file_name("a b/c"), "a_b_c");
    }

    #[test]
    fn tokens_are_sixty_four_hex_characters() {
        let g = GeneratedRegistration::new(&request("https://x"), "example.test").unwrap();
        for token in [&g.as_token, &g.hs_token] {
            assert_eq!(token.len(), 64);
            assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
        }
        assert_ne!(g.as_token, g.hs_token);
    }

    #[test]
    fn fingerprint_is_sha256_hex_first_eight() {
        let token = "0f".repeat(32);
        let digest = Sha256::digest(token.as_bytes());
        let expected: String = digest[..4].iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(fingerprint(&token), expected);
        assert_eq!(fingerprint(&token).len(), 8);
    }

    /// The full store path against a real seeded registration row: the
    /// migration ran, the files landed, the transport read returns the
    /// stored tokens — the acceptance's "the service reads the stored
    /// token" arm, at the store layer.
    #[test]
    fn issue_side_registration_end_to_end_at_the_store() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        let mut db = crate::DomainRepository::open(&state).unwrap();
        let registration = serde_json::from_value::<hagency_core::authority::Registration>(
            serde_json::json!({
                "fleetId": "hf_0123456789abcdef0123456789abcdef",
                "generation": 1,
                "serverName": "example.test",
                "receptionRoomId": "!reception:example.test",
                "representativeMxid": "@hf_0123456789abcdef0123456789abcdef_representative:example.test",
                "approvalBotMxid": "@approval:example.test",
            }),
        )
        .unwrap();
        db.register(&registration).unwrap();
        let issued = db
            .issue_side_registration(&request("http://127.0.0.1:13443///"), 1000)
            .unwrap();
        assert!(!issued.staged);
        assert_eq!(issued.registration_id, "hagency-example.test");
        let yaml = std::fs::read_to_string(state.join("registrations/example.test.yaml")).unwrap();
        assert!(yaml.contains("url: \"http://127.0.0.1:13443\"\n"));
        let as_token = yaml_field(&yaml, "as_token");
        let stored =
            String::from_utf8(std::fs::read(state.join("matrix.appservice_token")).unwrap())
                .unwrap();
        assert_eq!(stored, as_token);
        let credential = db
            .side_credential_for_transport("EXAMPLE.TEST")
            .unwrap()
            .expect("live credential");
        assert_eq!(credential.as_token, as_token);
        // The reissue stages and leaves the live token alone.
        let second = db
            .issue_side_registration(&request("http://127.0.0.1:14443"), 2000)
            .unwrap();
        assert!(second.staged);
        let after =
            String::from_utf8(std::fs::read(state.join("matrix.appservice_token")).unwrap())
                .unwrap();
        assert_eq!(after, as_token);
    }

    #[test]
    fn issue_side_registration_unknown_side_is_not_found() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        let mut db = crate::DomainRepository::open(&state).unwrap();
        assert!(matches!(
            db.issue_side_registration(&request("https://x"), 1000),
            Err(Error::NotFound)
        ));
    }

    fn yaml_field(yaml: &str, key: &str) -> String {
        yaml.lines()
            .find_map(|l| l.strip_prefix(&format!("{key}: ")))
            .expect("key present")
            .trim_matches('"')
            .to_owned()
    }
}
