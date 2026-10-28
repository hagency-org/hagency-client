//! Project-side credential install/verify (staged) and side lifecycle, TS
//! parity with `lib/project-side-store.js` (board #14). The record is the
//! side: one row per homeserver — the id IS the server name (ADR-016 decision
//! 1). The credential family lives ONLY in `credential`/`pending_credential`
//! JSON; every read of a side through this module goes through the allow-list
//! `SideRecord` projection (the same rule as TS `publicSide`), so no handler
//! can serialize the token columns verbatim. The store owns every guarantee —
//! shape validation, the staged-vs-live split, the access verdict, the
//! representative's server-must-match rule, the project room-uniqueness rule,
//! and the active-side removal refusal — exactly as the TS store does; the
//! routes add no guard of their own.
use super::DomainRepository;
use super::graphs;
use crate::Error;
use hagency_core::InvalidInput;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The two credential kinds of ADR-016 decision 2.
const CREDENTIAL_KINDS: [&str; 2] = ["appservice", "registrationToken"];

/// A credential's access verdict: a word, never a value.
const ACCESS_STATES: [&str; 5] = [
    "unverified",
    "accepted",
    "rejected",
    "unreachable",
    "blocked",
];

/// A server name: a DNS name or IP literal, optionally `:port`, lowercase.
/// Not a URL — the server name is an identity component; the API base URL is a
/// separate network location (TS `serverName`).
fn server_name(value: &str) -> Result<String, Error> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() || normalized.len() > 255 || normalized.contains('\0') {
        return Err(InvalidInput("server_name must be 1..255 characters").into());
    }
    let (host, port_ok) = match normalized.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() => (host, port_valid(port)),
        _ => (normalized.as_str(), true),
    };
    if !port_ok {
        return Err(InvalidInput("server_name must be a Matrix server name such as example.org or 127.0.0.1:8008, not a URL").into());
    }
    let valid = !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        && host
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    if !valid {
        return Err(InvalidInput("server_name must be a Matrix server name such as example.org or 127.0.0.1:8008, not a URL").into());
    }
    Ok(normalized)
}
fn port_valid(port: &str) -> bool {
    !port.is_empty()
        && port.len() <= 5
        && port.bytes().all(|b| b.is_ascii_digit())
        && port.parse::<u32>().is_ok()
}

fn text(value: &str, max: usize) -> Result<String, Error> {
    let normalized = value.trim().to_string();
    if normalized.is_empty() || normalized.len() > max || normalized.contains('\0') {
        return Err(InvalidInput("text is empty or too long").into());
    }
    Ok(normalized)
}

/// The stored credential. `appservice` carries `as_token`/`hs_token` and the
/// namespace claim; `registrationToken` carries the registration token and the
/// representative's own access token. THE VALUE IS A SECRET and never reaches
/// a projection — `SideRecord` carries only `credential_kind`/`has_credential`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credential {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hs_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_localpart: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representative_token: Option<String>,
}

impl Credential {
    /// Shared validation for trusted native project-connection maintenance.
    /// The returned value contains secrets and must remain outside script DTOs.
    pub fn from_web_configuration(value: &Value) -> Result<Self, Error> {
        credential(value)
    }
}

/// Validate a credential value and return it normalized (TS `credential()`).
/// The return value is a secret.
fn credential(value: &Value) -> Result<Credential, Error> {
    let object = value
        .as_object()
        .ok_or(InvalidInput("credential must be an object"))?;
    let kind = text(
        object
            .get("kind")
            .and_then(Value::as_str)
            .ok_or(InvalidInput("credential.kind must be 1..64 characters"))?,
        64,
    )?;
    if !CREDENTIAL_KINDS.contains(&kind.as_str()) {
        return Err(
            InvalidInput("credential.kind must be one of appservice, registrationToken").into(),
        );
    }
    let opt = |key: &str| -> Result<Option<String>, Error> {
        match object.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => text(s, 4096).map(Some),
            Some(_) => Err(InvalidInput("credential field must be a string").into()),
        }
    };
    if kind == "appservice" {
        Ok(Credential {
            kind,
            as_token: Some(text(
                object
                    .get("asToken")
                    .and_then(Value::as_str)
                    .ok_or(InvalidInput(
                        "credential.asToken must be 1..4096 characters",
                    ))?,
                4096,
            )?),
            hs_token: Some(text(
                object
                    .get("hsToken")
                    .and_then(Value::as_str)
                    .ok_or(InvalidInput(
                        "credential.hsToken must be 1..4096 characters",
                    ))?,
                4096,
            )?),
            url: match object.get("url") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) if s.trim().is_empty() => None,
                Some(Value::String(s)) => Some(text(s, 1024)?),
                Some(_) => return Err(InvalidInput("credential.url must be a string").into()),
            },
            namespace: Some(text(
                object
                    .get("namespace")
                    .and_then(Value::as_str)
                    .ok_or(InvalidInput(
                        "credential.namespace must be 1..255 characters",
                    ))?,
                255,
            )?),
            sender_localpart: Some(
                text(
                    object
                        .get("senderLocalpart")
                        .and_then(Value::as_str)
                        .ok_or({
                            InvalidInput("credential.senderLocalpart must be 1..255 characters")
                        })?,
                    255,
                )?
                .to_ascii_lowercase(),
            ),
            registration_token: None,
            representative_token: None,
        })
    } else {
        Ok(Credential {
            kind,
            as_token: None,
            hs_token: None,
            url: None,
            namespace: None,
            sender_localpart: None,
            registration_token: Some(text(
                object
                    .get("registrationToken")
                    .and_then(Value::as_str)
                    .ok_or({
                        InvalidInput("credential.registrationToken must be 1..4096 characters")
                    })?,
                4096,
            )?),
            representative_token: opt("representativeToken")?,
        })
    }
}

/// The representative's identity on a side (TS `record.representative`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Representative {
    pub mxid: String,
    pub localpart: String,
    pub observed_at: u64,
}

/// One project under a side (TS `record.projects[id]`): a name and a room, not
/// a second credential. (Named `SideProjectRecord` to avoid colliding with the
/// ADR-132 read projection's two-key `SideProject`.)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SideProjectRecord {
    pub id: String,
    pub name: String,
    pub room_id: Option<String>,
    pub note: Option<String>,
    pub archived: bool,
    pub archived_at: Option<u64>,
    pub created_at: u64,
    pub updated_at: u64,
}

/// What may leave the store — the TS `publicSide` allow-list projection. The
/// credential is ABSENT BY CONSTRUCTION: the only credential-shaped fields are
/// `credential_kind` and `has_credential`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SideRecord {
    pub id: String,
    pub label: String,
    pub server_name: String,
    pub api_base_url: Option<String>,
    pub credential_kind: Option<String>,
    pub has_credential: bool,
    pub awaiting_install: bool,
    pub awaiting_install_since: Option<u64>,
    pub sender_localpart: Option<String>,
    pub appservice_url: Option<String>,
    pub namespace: Option<String>,
    pub access_issued_at: Option<u64>,
    pub access_state: String,
    pub access_detail: Option<String>,
    pub access_checked_at: Option<u64>,
    pub allocated_tokens: Option<u64>,
    pub representative: Option<Representative>,
    pub projects: Vec<SideProjectRecord>,
    pub active: bool,
    pub created_at: u64,
    pub updated_at: u64,
}

type Row = (
    String,         // server_name
    String,         // label
    Option<String>, // api_base_url
    Option<String>, // credential (JSON)
    Option<String>, // pending_credential (JSON)
    Option<i64>,    // pending_issued_at
    Option<String>, // representative (JSON)
    String,         // access_state
    Option<String>, // access_detail
    Option<i64>,    // access_checked_at
    Option<i64>,    // access_issued_at
    Option<i64>,    // allocated_tokens
    i64,            // active
    i64,            // created_at
    i64,            // updated_at
);

fn side_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
        r.get(11)?,
        r.get(12)?,
        r.get(13)?,
        r.get(14)?,
    ))
}

fn parse_credential(json: &str) -> Result<Credential, Error> {
    serde_json::from_str(json).map_err(Error::from)
}

fn project_row(db: &Connection, server_name: &str) -> Result<Vec<SideProjectRecord>, Error> {
    let mut stmt = db.prepare(
        "SELECT id,name,room_id,note,archived,archived_at,created_at,updated_at \
         FROM side_projects WHERE server_name=?1 ORDER BY id",
    )?;
    let rows = stmt.query_map([server_name], |r| {
        Ok(SideProjectRecord {
            id: r.get(0)?,
            name: r.get(1)?,
            room_id: r.get(2)?,
            note: r.get(3)?,
            archived: r.get::<_, i64>(4)? != 0,
            archived_at: r.get(5)?,
            created_at: r.get::<_, i64>(6)? as u64,
            updated_at: r.get::<_, i64>(7)? as u64,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Error::from)
}

fn project(raw: Row, projects: Vec<SideProjectRecord>) -> Result<SideRecord, Error> {
    let (
        server_name,
        label,
        api_base_url,
        credential,
        pending_credential,
        pending_issued_at,
        representative,
        access_state,
        access_detail,
        access_checked_at,
        access_issued_at,
        allocated_tokens,
        active,
        created_at,
        updated_at,
    ) = raw;
    let live = match credential.as_deref() {
        Some(json) => Some(parse_credential(json)?),
        None => None,
    };
    let pending = pending_credential.is_some();
    Ok(SideRecord {
        id: server_name.clone(),
        label,
        server_name,
        api_base_url,
        credential_kind: live.as_ref().map(|c| c.kind.clone()),
        has_credential: live.is_some(),
        awaiting_install: pending,
        awaiting_install_since: pending_issued_at.map(|v| v as u64),
        sender_localpart: live.as_ref().and_then(|c| c.sender_localpart.clone()),
        appservice_url: live.as_ref().and_then(|c| c.url.clone()),
        namespace: live.as_ref().and_then(|c| c.namespace.clone()),
        access_issued_at: access_issued_at.map(|v| v as u64),
        access_state,
        access_detail,
        access_checked_at: access_checked_at.map(|v| v as u64),
        allocated_tokens: allocated_tokens.map(|v| v as u64),
        representative: representative
            .as_deref()
            .map(serde_json::from_str::<Representative>)
            .transpose()?,
        projects,
        active: active != 0,
        created_at: created_at as u64,
        updated_at: updated_at as u64,
    })
}

fn read_side(db: &Connection, id: &str) -> Result<Option<SideRecord>, Error> {
    let raw: Option<Row> = db
        .query_row(
            "SELECT server_name,label,api_base_url,credential,pending_credential,pending_issued_at,\
             representative,access_state,access_detail,access_checked_at,access_issued_at,\
             allocated_tokens,active,created_at,updated_at FROM side_records WHERE server_name=?1",
            [id],
            side_row,
        )
        .optional()?;
    match raw {
        None => Ok(None),
        Some(raw) => {
            let projects = project_row(db, id)?;
            Ok(Some(project(raw, projects)?))
        }
    }
}

/// Validate an mxid and its host against the side's server (TS `setRepresentative`).
fn representative(mxid: &str, server: &str, now: u64) -> Result<Representative, Error> {
    let mxid = text(mxid, 255)?;
    if !mxid.starts_with('@') || !mxid.contains(':') {
        return Err(InvalidInput("mxid must be a full Matrix MXID").into());
    }
    let host = mxid[mxid.find(':').expect("contains colon") + 1..].to_ascii_lowercase();
    if host != server {
        // `InvalidInput` carries only a static word; the dynamic host is
        // checked, not echoed (the route reports it via its own wording).
        return Err(
            InvalidInput("representative mxid must live on this project side's server").into(),
        );
    }
    let localpart = mxid[1..mxid.find(':').expect("contains colon")].to_string();
    Ok(Representative {
        mxid,
        localpart,
        observed_at: now,
    })
}

impl DomainRepository {
    /// Ensure the side row for a server name exists (label = server name, no
    /// credential, active). Called by the console create route, mirroring TS
    /// `upsertSide`'s create arm. Idempotent.
    pub fn ensure_side(&mut self, id: &str) -> Result<(), Error> {
        let id = server_name(id)?;
        let now = graphs::now_ms()?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO side_records(server_name,label,api_base_url,access_state,active,created_at,updated_at) \
             VALUES(?1,?1,NULL,'unverified',1,?2,?2) \
             ON CONFLICT(server_name) DO NOTHING",
            params![id, now as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Set the side's API base URL — the address the homeserver answers on
    /// (`verify` needs it to call whoami). TS stores it on `upsertSide`; the
    /// native create route (`register`) has no such field, so the lifecycle
    /// route records it here. Absent means "we do not know".
    pub fn set_api_base_url(
        &mut self,
        id: &str,
        api_base_url: Option<&str>,
    ) -> Result<Option<SideRecord>, Error> {
        let id = server_name(id)?;
        let api_base_url = match api_base_url {
            None => None,
            Some(url) => {
                let url = text(url, 1024)?;
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err(InvalidInput("api_base_url must be http or https").into());
                }
                Some(url.trim_end_matches('/').to_string())
            }
        };
        let now = graphs::now_ms()?;
        let changed = self.db.execute(
            "UPDATE side_records SET api_base_url=?2,updated_at=?3 WHERE server_name=?1",
            params![id, api_base_url, now as i64],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        read_side(&self.db, &id)
    }

    /// The side's public projection (TS `getSide` → `publicSide`).
    pub fn side(&self, id: &str) -> Result<Option<SideRecord>, Error> {
        let id = server_name(id)?;
        read_side(&self.db, &id)
    }

    /// The live credential, for the one caller that talks to the homeserver
    /// (TS `credentialFor`). A secret, never part of a projection.
    pub fn credential_for(&self, id: &str) -> Result<Option<Credential>, Error> {
        let id = server_name(id)?;
        let value: Option<Option<String>> = self
            .db
            .query_row(
                "SELECT credential FROM side_records WHERE server_name=?1",
                [&id],
                |r| r.get(0),
            )
            .optional()?;
        value.flatten().as_deref().map(parse_credential).transpose()
    }

    /// The staged credential waiting to be installed (TS `pendingCredentialFor`).
    pub fn pending_credential_for(&self, id: &str) -> Result<Option<Credential>, Error> {
        let id = server_name(id)?;
        let value: Option<Option<String>> = self
            .db
            .query_row(
                "SELECT pending_credential FROM side_records WHERE server_name=?1",
                [&id],
                |r| r.get(0),
            )
            .optional()?;
        value.flatten().as_deref().map(parse_credential).transpose()
    }

    /// Replace the credential alone (TS `setCredential`). `stage` keeps the old
    /// live credential and writes `pending_credential` WITHOUT touching
    /// `accessState`; non-stage replaces live, clears pending, resets the
    /// verdict and stamps `access_issued_at`. Returns the projection, never the
    /// value.
    pub fn set_credential(
        &mut self,
        id: &str,
        value: Option<Value>,
        stage: bool,
    ) -> Result<Option<SideRecord>, Error> {
        let id = server_name(id)?;
        let now = graphs::now_ms()?;
        let exists: bool = self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM side_records WHERE server_name=?1)",
            [&id],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(None);
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if stage && let Some(staged) = &value {
            // Guard the same way TS does: staging a FIRST credential would
            // leave the side unable to act while `hasCredential` says otherwise.
            let live: Option<Option<String>> = tx
                .query_row(
                    "SELECT credential FROM side_records WHERE server_name=?1",
                    [&id],
                    |r| r.get(0),
                )
                .optional()?;
            if live.flatten().is_some() {
                let credential = credential(staged)?;
                tx.execute(
                    "UPDATE side_records SET pending_credential=?2,pending_issued_at=?3,updated_at=?3 WHERE server_name=?1",
                    params![id, serde_json::to_string(&credential)?, now as i64],
                )?;
                tx.commit()?;
                return read_side(&self.db, &id);
            }
            // Fall through: staging a first credential is refused as a normal set.
        }
        let (credential_json, access_issued) = match value {
            Some(value) => {
                let credential = credential(&value)?;
                (Some(serde_json::to_string(&credential)?), now)
            }
            None => (None, 0),
        };
        tx.execute(
            "UPDATE side_records SET credential=?2,pending_credential=NULL,pending_issued_at=NULL,\
             access_state='unverified',access_detail=NULL,access_checked_at=NULL,\
             access_issued_at=CASE WHEN ?2 IS NULL THEN NULL ELSE ?3 END,updated_at=?3 \
             WHERE server_name=?1",
            params![id, credential_json, access_issued as i64],
        )?;
        tx.commit()?;
        read_side(&self.db, &id)
    }

    /// Make the staged credential the live one (TS `promotePendingCredential`),
    /// called only after the homeserver proved it accepts it.
    pub fn promote_pending_credential(&mut self, id: &str) -> Result<Option<SideRecord>, Error> {
        let id = server_name(id)?;
        let now = graphs::now_ms()?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE side_records SET credential=pending_credential,pending_credential=NULL,\
             pending_issued_at=NULL,access_issued_at=?2,updated_at=?2 \
             WHERE server_name=?1 AND pending_credential IS NOT NULL",
            params![id, now as i64],
        )?;
        tx.commit()?;
        if changed == 0 {
            return Ok(None);
        }
        read_side(&self.db, &id)
    }

    /// Record what the homeserver said (TS `observeAccess`).
    pub fn observe_access(
        &mut self,
        id: &str,
        state: &str,
        detail: Option<&str>,
    ) -> Result<Option<SideRecord>, Error> {
        let id = server_name(id)?;
        let state = text(state, 32)?;
        if !ACCESS_STATES.contains(&state.as_str()) {
            return Err(InvalidInput(
                "state must be one of unverified, accepted, rejected, unreachable, blocked",
            )
            .into());
        }
        let detail = match detail {
            None => None,
            Some(detail) => Some(text(detail, 1024)?),
        };
        let now = graphs::now_ms()?;
        let changed = self.db.execute(
            "UPDATE side_records SET access_state=?2,access_detail=?3,access_checked_at=?4,updated_at=?4 \
             WHERE server_name=?1",
            params![id, state, detail, now as i64],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        read_side(&self.db, &id)
    }

    /// Record the representative's identity (TS `setRepresentative`).
    pub fn set_representative(
        &mut self,
        id: &str,
        mxid: &str,
    ) -> Result<Option<SideRecord>, Error> {
        let id = server_name(id)?;
        let now = graphs::now_ms()?;
        let representative = representative(mxid, &id, now)?;
        let changed = self.db.execute(
            "UPDATE side_records SET representative=?2,updated_at=?3 WHERE server_name=?1",
            params![id, serde_json::to_string(&representative)?, now as i64],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        read_side(&self.db, &id)
    }

    /// Add or update a project under a side (TS `upsertProject`). The id is
    /// derived from the name (keeping letters in any script); a room may belong
    /// to one project only.
    pub fn upsert_project(
        &mut self,
        side: &str,
        input: &Value,
    ) -> Result<Option<SideProjectRecord>, Error> {
        let side_id = server_name(side)?;
        let exists: bool = self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM side_records WHERE server_name=?1)",
            [&side_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(None);
        }
        let object = input
            .as_object()
            .ok_or(InvalidInput("project must be an object"))?;
        let name = text(
            object
                .get("name")
                .and_then(Value::as_str)
                .ok_or(InvalidInput("project.name must be 1..255 characters"))?,
            255,
        )?;
        // The slug keeps letters in any script: `\p{L}\p{N}._-`, others become '-'.
        let slug_input = object
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(&name)
            .trim()
            .to_ascii_lowercase();
        let mut id = String::new();
        for ch in slug_input.chars() {
            if ch.is_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                id.push(ch);
            } else {
                id.push('-');
            }
        }
        let id = id.trim_matches('-').to_string();
        if id.is_empty() {
            return Err(InvalidInput("project.name must contain a letter or digit").into());
        }
        // A room-spelling we do not accept is refused, not dropped (TS): only
        // keys that clearly mean a room; an unrelated extra field is ignored.
        for key in object.keys() {
            if key.to_ascii_lowercase().contains("room")
                && !["room_id", "roomId", "room"].contains(&key.as_str())
            {
                return Err(
                    InvalidInput("project room must be sent as room_id, roomId or room").into(),
                );
            }
        }
        let room_raw = object
            .get("room_id")
            .or_else(|| object.get("roomId"))
            .or_else(|| object.get("room"))
            .cloned();
        let existing: Option<SideProjectRecord> = project_row(&self.db, &side_id)?
            .into_iter()
            .find(|p| p.id == id);
        let room_id = match room_raw {
            None | Some(Value::Null) => existing.as_ref().and_then(|p| p.room_id.clone()),
            Some(Value::String(s)) if s.trim().is_empty() => None,
            Some(Value::String(s)) => Some(text(&s, 255)?),
            Some(_) => return Err(InvalidInput("project.roomId must be a string").into()),
        };
        if let Some(room_id) = &room_id {
            if !room_id.starts_with('!') {
                return Err(
                    InvalidInput("project.roomId must be a room id starting with !").into(),
                );
            }
            let at = room_id.find(':');
            let host = at
                .map(|i| room_id[i + 1..].to_ascii_lowercase())
                .unwrap_or_default();
            if host != side_id {
                return Err(
                    InvalidInput("project.roomId must live on this project side's server").into(),
                );
            }
            let clash: Option<String> = self.db.query_row(
                "SELECT id FROM side_projects WHERE server_name=?1 AND room_id=?2 AND id<>?3 LIMIT 1",
                params![side_id, room_id, id],
                |r| r.get(0),
            ).optional()?;
            if clash.is_some() {
                return Err(Error::Conflict);
            }
        }
        let now = graphs::now_ms()?;
        let note = match object.get("note") {
            None => existing.as_ref().and_then(|p| p.note.clone()),
            Some(Value::Null) => None,
            Some(Value::String(s)) => Some(text(s, 1024)?),
            Some(_) => return Err(InvalidInput("project.note must be a string").into()),
        };
        let archived = existing.as_ref().map(|p| p.archived).unwrap_or(false);
        let archived_at = existing.as_ref().and_then(|p| p.archived_at);
        let created_at = existing.as_ref().map(|p| p.created_at).unwrap_or(now);
        self.db.execute(
            "INSERT INTO side_projects(server_name,id,name,room_id,note,archived,archived_at,created_at,updated_at) \
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) \
             ON CONFLICT(server_name,id) DO UPDATE SET name=excluded.name,room_id=excluded.room_id,\
             note=excluded.note,archived=excluded.archived,archived_at=excluded.archived_at,updated_at=excluded.updated_at",
            params![
                side_id,
                id,
                name,
                room_id,
                note,
                archived as i64,
                archived_at.map(|v| v as i64),
                created_at as i64,
                now as i64
            ],
        )?;
        self.db.execute(
            "UPDATE side_records SET updated_at=?2 WHERE server_name=?1",
            params![side_id, now as i64],
        )?;
        Ok(project_row(&self.db, &side_id)?
            .into_iter()
            .find(|p| p.id == id))
    }

    /// Archive (or un-archive) a project (TS `setProjectArchived`). No delete.
    pub fn set_project_archived(
        &mut self,
        side: &str,
        project_id: &str,
        archived: bool,
    ) -> Result<Option<SideProjectRecord>, Error> {
        let side_id = server_name(side)?;
        let project_id = project_id.trim().to_ascii_lowercase();
        let now = graphs::now_ms()?;
        let changed = self.db.execute(
            "UPDATE side_projects SET archived=?2,archived_at=CASE WHEN ?2 THEN ?3 ELSE NULL END,updated_at=?3 \
             WHERE server_name=?1 AND id=?4",
            params![side_id, archived as i64, now as i64, project_id],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        self.db.execute(
            "UPDATE side_records SET updated_at=?2 WHERE server_name=?1",
            params![side_id, now as i64],
        )?;
        Ok(project_row(&self.db, &side_id)?
            .into_iter()
            .find(|p| p.id == project_id))
    }

    /// Deactivate a side (TS `deactivateSide`).
    pub fn deactivate_side(&mut self, id: &str) -> Result<Option<SideRecord>, Error> {
        let id = server_name(id)?;
        let now = graphs::now_ms()?;
        let changed = self.db.execute(
            "UPDATE side_records SET active=0,updated_at=?2 WHERE server_name=?1",
            params![id, now as i64],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        read_side(&self.db, &id)
    }

    /// Reactivate a side (TS `reactivateSide`).
    pub fn reactivate_side(&mut self, id: &str) -> Result<Option<SideRecord>, Error> {
        let id = server_name(id)?;
        let now = graphs::now_ms()?;
        let changed = self.db.execute(
            "UPDATE side_records SET active=1,updated_at=?2 WHERE server_name=?1",
            params![id, now as i64],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        read_side(&self.db, &id)
    }

    /// Remove a side (TS `removeSide`). An active side is refused unless
    /// `force`; deactivate first. The retirement summary is served by the
    /// route from the pre-removal read, not reconstructed here.
    pub fn remove_side(&mut self, id: &str, force: bool) -> Result<(), Error> {
        let id = server_name(id)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active: Option<i64> = tx
            .query_row(
                "SELECT active FROM side_records WHERE server_name=?1",
                [&id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(active) = active else {
            return Ok(());
        };
        if active != 0 && !force {
            return Err(Error::State);
        }
        tx.execute("DELETE FROM side_projects WHERE server_name=?1", [&id])?;
        tx.execute("DELETE FROM side_records WHERE server_name=?1", [&id])?;
        tx.commit()?;
        Ok(())
    }
}
