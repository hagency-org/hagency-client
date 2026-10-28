//! Host-verified encrypted input bindings. Data alone never grants model authority.
//! The host must consume original AS/current-lease/SDK evidence before this call.
use crate::{
    Error, Ledger, Result, hash,
    inbox::{Dispatch, Limits, Receipt},
};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
pub(crate) const SCHEMA: &str = include_str!("encrypted_inputs/schema.sql");
pub(crate) const OBJECTS: &[&str] = &[
    "encrypted_inputs",
    "immutable_encrypted_input",
    "retain_encrypted_input",
];
/// Private owner data, never a serialized trust claim or execution permit.
pub struct Input {
    pub inbox_id: String,
    pub source_digest: String,
    pub agent_generation: i64,
    pub owner_direct: bool,
    pub ciphertext: Value,
    pub message: Value,
}
fn invalid() -> Error {
    Error::Invalid("encrypted input binding")
}
pub fn canonical(v: &Value) -> Result<String> {
    Ok(match v {
        Value::Object(o) => {
            let mut pairs = o.iter().collect::<Vec<_>>();
            pairs.sort_by_key(|(k, _)| *k);
            let mut out = Vec::new();
            for (k, v) in pairs {
                out.push(format!("{}:{}", serde_json::to_string(k)?, canonical(v)?));
            }
            format!("{{{}}}", out.join(","))
        }
        Value::Array(a) => format!(
            "[{}]",
            a.iter()
                .map(canonical)
                .collect::<Result<Vec<_>>>()?
                .join(",")
        ),
        _ => serde_json::to_string(v)?,
    })
}
fn seal_digest(input: &Input, d: &Dispatch) -> Result<String> {
    use base64::Engine;
    let c = &input.ciphertext["content"];
    let raw = c["ciphertext"].as_str().ok_or_else(invalid)?;
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(raw.trim_end_matches('='))
        .map_err(|_| invalid())?;
    let encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes);
    let session = c["session_id"]
        .as_str()
        .ok_or_else(invalid)?
        .trim_end_matches('=');
    Ok(format!(
        "{:x}",
        Sha256::digest(
            canonical(&json!([
                "agent-encrypted-seal-v1",
                d.agent_id,
                d.room_id,
                c["algorithm"],
                session,
                encoded
            ]))?
            .as_bytes()
        )
    ))
}
fn validate(input: &Input, d: &Dispatch) -> Result<(String, String, String, String, String)> {
    crate::key(&input.inbox_id)?;
    let c = &input.ciphertext;
    let m = &input.message;
    if input.agent_generation <= 0
        || !input.inbox_id.starts_with("enc_")
        || c["type"] != "m.room.encrypted"
        || m["type"] != "m.room.message"
        || c["event_id"] != d.event_id
        || m["event_id"] != d.event_id
        || c["room_id"] != d.room_id
        || m.get("room_id").is_some_and(|r| r != &d.room_id)
        || c["sender"] != d.requester_mxid
        || m["sender"] != d.requester_mxid
        || m.get("state_key").is_some()
        || m["unsigned"].get("redacted_because").is_some()
        || m["content"]["body"] != d.body
        || !matches!(
            m["content"]["msgtype"].as_str(),
            Some("m.text" | "m.file" | "m.image" | "m.audio" | "m.video")
        )
        || m["content"]["m.relates_to"]["rel_type"] == "m.replace"
    {
        return Err(invalid());
    }
    let expected = if m["content"]["m.relates_to"]["rel_type"] == "m.thread" {
        m["content"]["m.relates_to"]["event_id"]
            .as_str()
            .ok_or_else(invalid)?
    } else if input.owner_direct {
        &d.room_id
    } else {
        &d.event_id
    };
    if d.thread_root != expected {
        return Err(invalid());
    }
    let ciphertext = canonical(c)?;
    let message = canonical(m)?;
    if ciphertext.len() > 131072 || message.len() > 131072 {
        return Err(invalid());
    }
    let digest = format!(
        "{:x}",
        Sha256::digest(canonical(&json!(["agent-encrypted-inbox-v1", c]))?.as_bytes())
    );
    if digest != input.source_digest {
        return Err(invalid());
    }
    let mut transport = d.clone();
    transport.body = ciphertext.clone();
    Ok((
        ciphertext,
        message.clone(),
        hash(&message)?,
        transport.digest()?,
        seal_digest(input, d)?,
    ))
}
pub(crate) fn record(
    tx: &Transaction<'_>,
    d: &Dispatch,
    input: Option<&Input>,
    plain_digest: &str,
) -> Result<()> {
    type Stored = (
        String,
        String,
        i64,
        bool,
        Option<String>,
        Option<String>,
        String,
        String,
        String,
        String,
    );
    let old:Option<Stored>=tx.query_row("SELECT inbox_id,source_digest,agent_generation,owner_direct,ciphertext,message,message_digest,plaintext_digest,transport_digest,seal_digest FROM encrypted_inputs WHERE dispatch_id=?",[&d.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?))).optional()?;
    let Some(input) = input else {
        return if old.is_none() {
            Ok(())
        } else {
            Err(Error::Conflict)
        };
    };
    let (cipher, message, message_digest, transport_digest, seal) = validate(input, d)?;
    if let Some((id, source, generation, direct, old_cipher, old_message, md, pd, td, old_seal)) =
        old
    {
        if id != input.inbox_id
            || source != input.source_digest
            || generation != input.agent_generation
            || direct != input.owner_direct
            || md != message_digest
            || pd != plain_digest
            || td != transport_digest
            || old_seal != seal
            || old_cipher.as_ref().is_some_and(|v| v != &cipher)
            || old_message.as_ref().is_some_and(|v| v != &message)
        {
            return Err(Error::Conflict);
        }
        return Ok(());
    }
    if tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM local_inbox WHERE id=?)",
        [&d.id],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(Error::Conflict);
    }
    tx.execute(
        "INSERT INTO encrypted_inputs VALUES(?,?,?,?,?,?,?,?,?,?,?)",
        (
            &d.id,
            &input.inbox_id,
            &input.source_digest,
            input.agent_generation,
            input.owner_direct,
            seal,
            cipher,
            message,
            message_digest,
            plain_digest,
            transport_digest,
        ),
    )?;
    Ok(())
}
pub(crate) fn history_digest(tx: &Transaction<'_>, id: &str, local: &str) -> Result<String> {
    let row: Option<(String, String)> = tx
        .query_row(
            "SELECT plaintext_digest,transport_digest FROM encrypted_inputs WHERE dispatch_id=?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match row {
        Some((plain, transport)) if plain == local => Ok(transport),
        Some(_) => Err(Error::Conflict),
        None => Ok(local.into()),
    }
}
pub(crate) fn require_mode(tx: &Transaction<'_>, id: &str, encrypted: bool) -> Result<()> {
    let present: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM encrypted_inputs WHERE dispatch_id=?)",
        [id],
        |r| r.get(0),
    )?;
    if present != encrypted {
        return Err(Error::Invalid(
            "encrypted input requires encrypted execution transport",
        ));
    }
    Ok(())
}
impl Ledger {
    pub fn encrypted_input(&self, owner: &str, id: &str) -> Result<Option<Input>> {
        self.owner(owner)?;
        self.db.query_row("SELECT inbox_id,source_digest,agent_generation,owner_direct,ciphertext,message FROM encrypted_inputs WHERE dispatch_id=? AND ciphertext IS NOT NULL",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,bool>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?))).optional()?.map(|(inbox_id,source_digest,agent_generation,owner_direct,c,m)|Ok(Input{inbox_id,source_digest,agent_generation,owner_direct,ciphertext:serde_json::from_str(&c)?,message:serde_json::from_str(&m)?})).transpose()
    }
}
pub(crate) fn compact(tx: &rusqlite::Connection) -> Result<()> {
    tx.execute("UPDATE encrypted_inputs SET ciphertext=NULL,message=NULL WHERE ciphertext IS NOT NULL AND EXISTS(SELECT 1 FROM local_inbox i WHERE i.id=encrypted_inputs.dispatch_id AND i.state IN ('replied','failed','rejected') AND json_extract(i.envelope,'$.body')='')",[])?;
    Ok(())
}
impl Ledger {
    pub fn receive_encrypted_dispatch(
        &mut self,
        owner: &str,
        event: &Dispatch,
        input: &Input,
        limits: Limits,
        now: i64,
    ) -> Result<Receipt> {
        self.receive_dispatch_kind(owner, event, Some(input), limits, now)
    }
    pub fn encrypted_input_transport_digest(
        &self,
        owner: &str,
        id: &str,
    ) -> Result<Option<String>> {
        self.owner(owner)?;
        crate::key(id)?;
        Ok(self
            .db
            .query_row(
                "SELECT transport_digest FROM encrypted_inputs WHERE dispatch_id=?",
                [id],
                |r| r.get(0),
            )
            .optional()?)
    }
}

pub(crate) fn validate_snapshot(db: &rusqlite::Connection) -> Result<()> {
    let present: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='encrypted_inputs')",
        [],
        |r| r.get(0),
    )?;
    if !present {
        return Ok(());
    }
    let mut q=db.prepare("SELECT e.inbox_id,e.source_digest,e.agent_generation,e.owner_direct,e.ciphertext,e.message,e.message_digest,e.plaintext_digest,e.transport_digest,i.envelope,i.immutable_digest,i.state,e.seal_digest FROM encrypted_inputs e JOIN local_inbox i ON i.id=e.dispatch_id")?;
    let rows = q.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, bool>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?,
            r.get::<_, String>(8)?,
            r.get::<_, String>(9)?,
            r.get::<_, String>(10)?,
            r.get::<_, String>(11)?,
            r.get::<_, String>(12)?,
        ))
    })?;
    for row in rows {
        let (
            id,
            source,
            generation,
            direct,
            cipher,
            message,
            md,
            pd,
            td,
            encoded,
            immutable,
            state,
            seal,
        ) = row?;
        let d: Dispatch = serde_json::from_str(&encoded)?;
        if pd != immutable {
            return Err(invalid());
        }
        match (cipher, message) {
            (Some(c), Some(m)) => {
                let i = Input {
                    inbox_id: id,
                    source_digest: source,
                    agent_generation: generation,
                    owner_direct: direct,
                    ciphertext: serde_json::from_str(&c)?,
                    message: serde_json::from_str(&m)?,
                };
                let (_, _, actual_message, actual_transport, actual_seal) = validate(&i, &d)?;
                if d.digest()? != pd
                    || actual_message != md
                    || actual_transport != td
                    || actual_seal != seal
                {
                    return Err(invalid());
                }
            }
            (None, None)
                if d.body.is_empty()
                    && matches!(state.as_str(), "replied" | "failed" | "rejected") => {}
            _ => return Err(invalid()),
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests;
