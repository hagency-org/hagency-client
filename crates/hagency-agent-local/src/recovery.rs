//! Stopped-dispatch inspection and explicit owner dispositions. Original execution
//! outcomes never change; a continuation is a new owner message, armed only once.
use crate::{
    Error, Ledger, Result, Scope, hash,
    inbox::{Record, State},
    json, key,
};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Component, Path},
};
pub(crate) const SCHEMA: &str = include_str!("recovery/schema.sql");
pub(crate) const OBJECTS: &[&str] = &[
    "execution_stops",
    "retain_execution_stop",
    "immutable_execution_stop",
    "execution_inputs",
    "retain_execution_input",
    "immutable_execution_input",
    "execution_workspaces",
    "retain_execution_workspace",
    "immutable_execution_workspace",
    "recovery_reviews",
    "recovery_review_dispatch",
    "retain_recovery_review",
    "immutable_recovery_review",
    "recovery_actions",
    "recovery_action_review",
    "retain_recovery_action",
    "immutable_recovery_action",
];
const MAX_FILE: u64 = 8 * 1024 * 1024;
const MAX_FILES: usize = 32;
const MAX_TOTAL: u64 = 32 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileEvidence {
    pub path: String,
    pub status: String,
    pub bytes: Option<u64>,
    pub sha256: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub dispatch_id: String,
    pub scope: Scope,
    pub immutable_digest: String,
    pub state: State,
    pub execution_id: Option<String>,
    pub input: String,
    pub reply: Option<String>,
    pub workspace: Option<String>,
    pub requires_tree: bool,
    pub stop_scope: Option<String>,
    pub files: Vec<FileEvidence>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub id: String,
    pub digest: String,
    pub observed_at: i64,
    pub expires: i64,
    pub evidence: Evidence,
    pub actions: Vec<Action>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stopped {
    pub id: String,
    pub received_at: i64,
    pub scope: Scope,
    pub state: State,
    pub execution_id: Option<String>,
    pub input_preview: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub records: Vec<Stopped>,
    pub next_cursor: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "choice", rename_all = "snake_case", deny_unknown_fields)]
pub enum Disposition {
    KeepBlocked { note: String },
    OwnerConfirmed { note: String },
    Continue { note: String, instruction: String },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub review_id: String,
    pub review_digest: String,
    pub disposition: Disposition,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub id: String,
    pub input: Input,
    pub deciding_device: String,
    pub observed_at: i64,
    pub phase: String,
    pub event_id: Option<String>,
}
/// Not Clone/Serialize: only the first committed decision can attempt a send.
/// Losing this custody cannot rearm the durable send_possible record.
pub struct Continuation {
    id: String,
    pub transaction_id: String,
    pub scope: Scope,
    pub instruction: String,
}
pub struct Decided {
    pub action: Action,
    pub continuation: Option<Continuation>,
}
fn stopped(db: &Connection, owner: &str, agent: &str, id: &str) -> Result<Record> {
    key(agent)?;
    key(id)?;
    let exists:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM local_inbox i JOIN bindings b ON i.binding=b.id WHERE i.id=? AND b.agent=?)",(id,agent),|r|r.get(0))?;
    if !exists {
        return Err(Error::Unauthorized);
    }
    let (envelope, state, execution_id, reply): (String, String, Option<String>, Option<String>) =
        db.query_row(
            "SELECT envelope,state,execution_id,reply FROM local_inbox WHERE id=?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
    let mut dispatch: crate::inbox::Dispatch = serde_json::from_str(&envelope)?;
    if dispatch.body.is_empty() {
        dispatch.body = db
            .query_row(
                "SELECT body FROM execution_inputs WHERE dispatch=?",
                [id],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_default();
    }
    if dispatch.agent_id != agent {
        return Err(Error::Unauthorized);
    }
    crate::verify(db, &dispatch.scope())?;
    let _ = owner;
    let state = match state.as_str() {
        "unknown" => State::Unknown,
        "failed" => State::Failed,
        _ => return Err(Error::Conflict),
    };
    Ok(Record {
        dispatch,
        state,
        execution_id,
        reply,
    })
}
fn random_id() -> Result<String> {
    let mut nonce = [0; 32];
    getrandom::fill(&mut nonce).map_err(|_| Error::Invalid("recovery entropy"))?;
    Ok(format!("{:x}", Sha256::digest(nonce)))
}
fn paths_valid(paths: &[String]) -> Result<()> {
    if paths.len() > MAX_FILES {
        return Err(Error::Invalid("recovery file capacity"));
    }
    let mut unique = std::collections::BTreeSet::new();
    for path in paths {
        if path.is_empty()
            || path.len() > 4096
            || path.contains(['\0', '\\', ':'])
            || Path::new(path)
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            || !unique.insert(path)
        {
            return Err(Error::Invalid("relative recovery path"));
        }
    }
    Ok(())
}
fn files(workspace: Option<&str>, paths: &[String]) -> Vec<FileEvidence> {
    let dir = workspace
        .and_then(|p| cap_std::fs::Dir::open_ambient_dir(p, cap_std::ambient_authority()).ok());
    let mut total = 0;
    paths
        .iter()
        .map(|path| {
            let mut evidence = FileEvidence {
                path: path.clone(),
                status: "unavailable".into(),
                bytes: None,
                sha256: None,
            };
            let Some(dir) = &dir else {
                return evidence;
            };
            match dir.symlink_metadata(path) {
                Ok(m) if m.file_type().is_symlink() => {
                    evidence.status = "refused".into();
                    return evidence;
                }
                Ok(m) if !m.is_file() => {
                    evidence.status = "not_regular".into();
                    return evidence;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    evidence.status = "missing".into();
                    return evidence;
                }
                Err(_) => return evidence,
                _ => {}
            }
            let mut options = cap_std::fs::OpenOptions::new();
            options.read(true).follow(FollowSymlinks::No).nonblock(true);
            let Ok(file) = dir.open_with(path, &options) else {
                evidence.status = "refused".into();
                return evidence;
            };
            let Ok(before) = file.metadata() else {
                return evidence;
            };
            if !before.is_file() {
                evidence.status = "not_regular".into();
                return evidence;
            }
            if before.len() > MAX_FILE || total + before.len() > MAX_TOTAL {
                evidence.status = "too_large".into();
                return evidence;
            }
            let mut data = Vec::new();
            let mut limited = (&file).take(MAX_FILE + 1);
            if limited.read_to_end(&mut data).is_err() {
                return evidence;
            }
            total += data.len() as u64;
            let Ok(after) = file.metadata() else {
                return evidence;
            };
            if data.len() as u64 != before.len()
                || before.len() != after.len()
                || before.modified().ok() != after.modified().ok()
            {
                evidence.status = "changed".into();
                return evidence;
            }
            evidence.status = "hashed".into();
            evidence.bytes = Some(data.len() as u64);
            evidence.sha256 = Some(format!("{:x}", Sha256::digest(&data)));
            evidence
        })
        .collect()
}
impl Ledger {
    pub fn record_execution_workspace(
        &mut self,
        owner: &str,
        dispatch: &str,
        execution: &str,
        workspace: &Path,
        requires_tree: bool,
        now: i64,
    ) -> Result<()> {
        self.owner(owner)?;
        key(dispatch)?;
        key(execution)?;
        let root = workspace
            .canonicalize()
            .map_err(|_| Error::Invalid("execution workspace unavailable"))?;
        if !root.is_dir() || now < 0 {
            return Err(Error::Invalid("execution workspace"));
        }
        let root = root
            .to_str()
            .ok_or(Error::Invalid("execution workspace encoding"))?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM local_inbox WHERE id=? AND execution_id=? AND state='running')",(dispatch,execution),|r|r.get(0))?;
        if !valid {
            return Err(Error::Unauthorized);
        }
        let existing: Option<(String, String, bool)> = tx
            .query_row(
                "SELECT execution_id,workspace,requires_tree FROM execution_workspaces WHERE dispatch=?",
                [dispatch],
                |r| Ok((r.get(0)?, r.get(1)?,r.get(2)?)),
            )
            .optional()?;
        if let Some((e, w, tree)) = existing {
            if e != execution || w != root || tree != requires_tree {
                return Err(Error::Conflict);
            }
        } else {
            tx.execute(
                "INSERT INTO execution_workspaces VALUES (?,?,?,?,?)",
                (dispatch, execution, root, requires_tree, now),
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Called only after the original provider stop returned an observed result.
    pub fn record_execution_stop(
        &mut self,
        owner: &str,
        dispatch: &str,
        execution: &str,
        scope: &str,
        now: i64,
    ) -> Result<()> {
        self.owner(owner)?;
        key(dispatch)?;
        key(execution)?;
        if now < 0 || !matches!(scope, "leader" | "whole_tree") {
            return Err(Error::Invalid("execution stop"));
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !tx.query_row("SELECT EXISTS(SELECT 1 FROM local_inbox WHERE id=? AND execution_id=? AND state='running')",(dispatch,execution),|r|r.get::<_,bool>(0))? {return Err(Error::Unauthorized);}
        let old: Option<(String, String)> = tx
            .query_row(
                "SELECT execution_id,scope FROM execution_stops WHERE dispatch=?",
                [dispatch],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((e, s)) = old {
            if e != execution || s != scope {
                return Err(Error::Conflict);
            }
        } else {
            tx.execute(
                "INSERT INTO execution_stops VALUES (?,?,?,?)",
                (dispatch, execution, scope, now),
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn stopped_history(&self, owner: &str, agent: &str, cursor: Option<&str>) -> Result<Page> {
        self.owner(owner)?;
        key(agent)?;
        let before: Option<i64> = if let Some(id) = cursor {
            key(id)?;
            Some(self.db.query_row("SELECT i.received_at FROM local_inbox i JOIN bindings b ON i.binding=b.id WHERE i.id=? AND b.agent=? AND i.state IN ('unknown','failed')",(id,agent),|r|r.get(0)).optional()?.ok_or(Error::Unauthorized)?)
        } else {
            None
        };
        let mut stmt=self.db.prepare("SELECT i.id,i.received_at FROM local_inbox i JOIN bindings b ON i.binding=b.id WHERE b.agent=?1 AND i.state IN ('unknown','failed') AND (?2 IS NULL OR i.received_at<?2 OR (i.received_at=?2 AND i.id<?3)) ORDER BY i.received_at DESC,i.id DESC LIMIT 101")?;
        let mut rows = stmt.query((agent, before, cursor))?;
        let mut records = Vec::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let r = stopped(&self.db, owner, agent, &id)?;
            records.push(Stopped {
                id,
                received_at: row.get(1)?,
                scope: r.dispatch.scope(),
                state: r.state,
                execution_id: r.execution_id,
                input_preview: r.dispatch.body.chars().take(240).collect(),
            });
        }
        let more = records.len() > 100;
        records.truncate(100);
        let next_cursor = more.then(|| records.last().unwrap().id.clone());
        Ok(Page {
            records,
            next_cursor,
        })
    }
    pub fn inspect_recovery(
        &mut self,
        owner: &str,
        agent: &str,
        dispatch: &str,
        paths: &[String],
        now: i64,
    ) -> Result<Review> {
        self.owner(owner)?;
        paths_valid(paths)?;
        if !(0..=i64::MAX - 300).contains(&now) {
            return Err(Error::Invalid("recovery time"));
        }
        let original = stopped(&self.db, owner, agent, dispatch)?;
        let workspace: Option<String> = self
            .db
            .query_row(
                "SELECT workspace FROM execution_workspaces WHERE dispatch=? AND execution_id=?",
                (dispatch, &original.execution_id),
                |r| r.get(0),
            )
            .optional()?;
        let immutable_digest = self.db.query_row(
            "SELECT immutable_digest FROM local_inbox WHERE id=?",
            [dispatch],
            |r| r.get(0),
        )?;
        let requires_tree = self
            .db
            .query_row(
                "SELECT requires_tree FROM execution_workspaces WHERE dispatch=?",
                [dispatch],
                |r| r.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false);
        let stop_scope = self
            .db
            .query_row(
                "SELECT scope FROM execution_stops WHERE dispatch=? AND execution_id=?",
                (dispatch, &original.execution_id),
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        let files = files(workspace.as_deref(), paths);
        let evidence = Evidence {
            dispatch_id: dispatch.into(),
            scope: original.dispatch.scope(),
            immutable_digest,
            state: original.state,
            execution_id: original.execution_id,
            input: original.dispatch.body,
            reply: original.reply,
            workspace,
            requires_tree,
            stop_scope,
            files,
        };
        let id = random_id()?;
        let digest = hash(&evidence)?;
        self.db.execute(
            "INSERT INTO recovery_reviews VALUES (?,?,?,?,?,?,?)",
            (
                &id,
                dispatch,
                agent,
                &digest,
                json(&evidence)?,
                now,
                now + 300,
            ),
        )?;
        Ok(Review {
            id,
            digest,
            observed_at: now,
            expires: now + 300,
            evidence,
            actions: self.recovery_actions(owner, agent, dispatch)?,
        })
    }
    pub fn has_recovery_action(&self, owner: &str, agent: &str, id: &str) -> Result<bool> {
        self.owner(owner)?;
        key(agent)?;
        key(id)?;
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM recovery_actions WHERE id=? AND agent=?)",
            (id, agent),
            |r| r.get(0),
        )?)
    }
    pub fn recovery_review(&self, owner: &str, agent: &str, id: &str) -> Result<Review> {
        self.owner(owner)?;
        key(agent)?;
        key(id)?;
        let (digest,evidence,at,expires):(String,String,i64,i64)=self.db.query_row("SELECT digest,evidence,created_at,expires FROM recovery_reviews WHERE id=? AND agent=?",(id,agent),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?.ok_or(Error::Unauthorized)?;
        let evidence: Evidence = serde_json::from_str(&evidence)?;
        let actions = self.recovery_actions(owner, agent, &evidence.dispatch_id)?;
        Ok(Review {
            id: id.into(),
            digest,
            observed_at: at,
            expires,
            evidence,
            actions,
        })
    }
    pub fn recovery_actions(
        &self,
        owner: &str,
        agent: &str,
        dispatch: &str,
    ) -> Result<Vec<Action>> {
        self.owner(owner)?;
        stopped(&self.db, owner, agent, dispatch)?;
        let mut stmt=self.db.prepare("SELECT a.id FROM recovery_actions a JOIN recovery_reviews r ON a.review_id=r.id WHERE r.dispatch=? AND a.agent=? ORDER BY a.created_at DESC,a.id DESC LIMIT 100")?;
        let ids = stmt
            .query_map((dispatch, agent), |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids.iter().map(|id| action(&self.db, id)).collect()
    }
    pub fn decide_recovery(
        &mut self,
        owner: &str,
        agent: &str,
        id: &str,
        input: &Input,
        device: &str,
        now: i64,
    ) -> Result<Decided> {
        self.owner(owner)?;
        key(agent)?;
        key(id)?;
        key(device)?;
        key(&input.review_id)?;
        if id.len() > 128
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err(Error::Invalid("recovery action identity"));
        }
        if input.review_digest.len() != 64
            || !input.review_digest.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(Error::Invalid("recovery digest"));
        }
        let encoded = json(input)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some((stored, original_agent)) = tx
            .query_row(
                "SELECT input,agent FROM recovery_actions WHERE id=?",
                [id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?
        {
            if stored != encoded || original_agent != agent {
                return Err(Error::Conflict);
            }
            return Ok(Decided {
                action: action(&tx, id)?,
                continuation: None,
            });
        }
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM recovery_actions WHERE review_id=?)",
            [&input.review_id],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(Error::Conflict);
        }
        let (dispatch,digest,evidence,expires):(String,String,String,i64)=tx.query_row("SELECT dispatch,digest,evidence,expires FROM recovery_reviews WHERE id=? AND agent=?",(&input.review_id,agent),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?.ok_or(Error::Unauthorized)?;
        if digest != input.review_digest || now < 0 || now >= expires {
            return Err(Error::Conflict);
        }
        let review: Evidence = serde_json::from_str(&evidence)?;
        let paths = review
            .files
            .iter()
            .map(|f| f.path.clone())
            .collect::<Vec<_>>();
        if files(review.workspace.as_deref(), &paths) != review.files {
            return Err(Error::Conflict);
        }
        let original = stopped(&tx, owner, agent, &dispatch)?;
        if original.execution_id != review.execution_id
            || original.state != review.state
            || original.dispatch.scope() != review.scope
        {
            return Err(Error::Conflict);
        }
        let (note, instruction) = match &input.disposition {
            Disposition::KeepBlocked { note } | Disposition::OwnerConfirmed { note } => {
                (note, None)
            }
            Disposition::Continue { note, instruction } => (note, Some(instruction)),
        };
        if note.trim().is_empty()
            || note.len() > 8192
            || instruction.is_some_and(|s| s.trim().is_empty() || s.len() > 32768)
        {
            return Err(Error::Invalid("recovery disposition"));
        }
        if instruction.is_some()
            && !(review.stop_scope.as_deref() == Some("whole_tree")
                || !review.requires_tree && review.stop_scope.as_deref() == Some("leader"))
        {
            return Err(Error::Denied);
        }
        let phase = if instruction.is_some() {
            "send_possible"
        } else {
            "recorded"
        };
        tx.execute(
            "INSERT INTO recovery_actions VALUES (?,?,?,?,?,?,?,NULL)",
            (id, &input.review_id, agent, encoded, device, now, phase),
        )?;
        let result=Decided{action:action(&tx,id)?,continuation:instruction.map(|instruction|Continuation{id:id.into(),transaction_id:format!("hagency-continue-{}",hash(&(owner,agent,id)).expect("string tuple")),scope:review.scope,instruction:format!("Explicit owner continuation of dispatch {} (original outcome remains {:?}). Inspect the existing effects before acting.\n\n{}",dispatch,review.state,instruction)})};
        tx.commit()?;
        Ok(result)
    }
    pub fn settle_continuation(
        &mut self,
        owner: &str,
        continuation: Continuation,
        event: Option<&str>,
    ) -> Result<Action> {
        self.owner(owner)?;
        if event.is_some_and(|e| !e.starts_with('$') || key(e).is_err()) {
            return Err(Error::Invalid("continuation event"));
        }
        self.db.execute(
            "UPDATE recovery_actions SET phase=?,event_id=? WHERE id=? AND phase='send_possible'",
            (
                if event.is_some() { "sent" } else { "unknown" },
                event,
                &continuation.id,
            ),
        )?;
        action(&self.db, &continuation.id)
    }
}
pub(crate) fn preserve_inputs(db: &Connection) -> Result<()> {
    db.execute("INSERT OR IGNORE INTO execution_inputs SELECT id,json_extract(envelope,'$.body') FROM local_inbox WHERE state IN ('unknown','failed') AND length(json_extract(envelope,'$.body'))>0",[])?;
    Ok(())
}
fn action(db: &Connection, id: &str) -> Result<Action> {
    let (input,device,at,phase,event):(String,String,i64,String,Option<String>)=db.query_row("SELECT input,deciding_device,created_at,phase,event_id FROM recovery_actions WHERE id=?",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
    Ok(Action {
        id: id.into(),
        input: serde_json::from_str(&input)?,
        deciding_device: device,
        observed_at: at,
        phase,
        event_id: event,
    })
}
#[cfg(test)]
mod tests;
