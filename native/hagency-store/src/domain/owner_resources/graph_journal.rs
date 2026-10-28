//! Native graph transactions and a durable, non-deliverable outbox. These rows
//! neither adopt legacy documents nor authorize runtime or Matrix execution.
use super::*;
use hagency_core::project::{hash as bytes_digest, identifier};
mod recipients;
pub use recipients::GraphRecipientBinding;

#[cfg(test)]
mod tests;

const MAX_DOCUMENT: usize = 4 * 1024 * 1024;
const MAX_MESSAGES: u64 = 256;
const MAX_MESSAGE_BYTES: u64 = 64 * 1024 * 1024;

/// Trusted native input; never deserialized from a mini-app request. The input
/// digest must cover the complete transition payload, including its operation.
#[derive(Clone, Debug)]
pub struct GraphJournalCommand {
    pub command_id: String,
    pub scope_id: String,
    pub graph_id: String,
    pub expected_revision: u64,
    pub input_digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphJournalRecord {
    pub revision: u64,
    pub document: Vec<u8>,
}

/// A bounded identity/revision index. Documents are read individually on the
/// native worker; listing never allocates sixteen four-MiB graph documents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphJournalRecordIndex {
    pub graph_id: String,
    pub revision: u64,
    pub document_bytes: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphJournalRecordPage {
    pub items: Vec<GraphJournalRecordIndex>,
    pub next: Option<String>,
}

/// Counts describe effects of this command, not current delivery state. Replay
/// returns this original result even after later cancellation or reopening.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphJournalReceipt {
    pub command_id: String,
    pub scope_id: String,
    pub graph_id: String,
    pub revision: u64,
    pub document_digest: String,
    pub queued_messages: u64,
    pub suppressed_messages: u64,
}

#[derive(Clone)]
struct Authority {
    registry: OwnerResourceRegistry,
    profile: ProfileAuthority,
}
impl Authority {
    fn check(&self) -> Result<(), Error> {
        self.profile.check()?;
        if self.registry.0.closed.load(Ordering::Acquire) {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
    fn check_db(&self, db: &Connection) -> Result<(), Error> {
        self.check()?;
        let stored: String = db.query_row(
            "SELECT id FROM owner_resource_identity WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        if stored != self.registry.0.id {
            return Err(Error::LocalAuthority);
        }
        self.check()
    }
}

/// Exists only inside one SQLite transaction. The callback cannot commit or
/// dispatch its rows. Any writer error poisons the whole transition, even when
/// the caller catches that error and attempts to return a graph document.
pub struct GraphJournalWriter<'a> {
    db: &'a Connection,
    authority: &'a Authority,
    scope: &'a str,
    graph: &'a str,
    revision: u64,
    queued: u64,
    suppressed: u64,
    failed: bool,
}
impl GraphJournalWriter<'_> {
    /// A graph node gets exactly one immutable native message identity. A
    /// duplicate (including a suppressed message) never invokes `build` again.
    pub fn enqueue(
        &mut self,
        node_id: &str,
        build: impl FnOnce(&str) -> Result<Vec<u8>, Error>,
    ) -> Result<String, Error> {
        let result = self.enqueue_inner(node_id, build);
        self.failed |= result.is_err();
        result
    }
    fn enqueue_inner(
        &mut self,
        node: &str,
        build: impl FnOnce(&str) -> Result<Vec<u8>, Error>,
    ) -> Result<String, Error> {
        self.authority.check_db(self.db)?;
        exact_id(node)?;
        let a = self.authority;
        let id = canonical::digest(
            &json!({"kind":"operator_graph_message_v1","registry":a.registry.0.id,"profile":a.profile.digest,"owner":a.profile.account,"scope":self.scope,"graph":self.graph,"node":node}),
        )?;
        let existing: Option<String> = self.db.query_row(
            "SELECT message_id FROM owner_operator_graph_outbox WHERE registry_id=?1 AND profile=?2 AND owner_mxid=?3 AND scope_id=?4 AND graph_id=?5 AND node_id=?6",
            params![a.registry.0.id,a.profile.digest,a.profile.account,self.scope,self.graph,node],
            |r| r.get(0),
        ).optional()?;
        if let Some(existing) = existing {
            if existing != id {
                return Err(Error::Quarantined);
            }
            a.check_db(self.db)?;
            return Ok(existing);
        }
        let (count, bytes): (u64, u64) = self.db.query_row(
            "SELECT COUNT(*),COALESCE(SUM(length(payload)),0) FROM owner_operator_graph_outbox WHERE registry_id=?1 AND profile=?2 AND owner_mxid=?3 AND scope_id=?4 AND graph_id=?5",
            params![a.registry.0.id,a.profile.digest,a.profile.account,self.scope,self.graph],
            |r| Ok((r.get(0)?,r.get(1)?)),
        )?;
        if count >= MAX_MESSAGES {
            return Err(Error::Capacity);
        }
        a.check_db(self.db)?;
        let built = build(&id);
        a.check_db(self.db)?;
        let payload = built?;
        if payload.len() > MAX_DOCUMENT || bytes + payload.len() as u64 > MAX_MESSAGE_BYTES {
            return Err(Error::Capacity);
        }
        self.db.execute(
            "INSERT INTO owner_operator_graph_outbox VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'pending_binding')",
            params![a.registry.0.id,a.profile.digest,a.profile.account,self.scope,self.graph,node,id,payload,bytes_digest(&payload)],
        )?;
        self.queued += 1;
        a.check_db(self.db)?;
        Ok(id)
    }
    /// Cancellation retains message IDs and bytes; it never marks delivery.
    pub fn suppress_pending(&mut self) -> Result<(), Error> {
        let result = self.suppress_inner();
        self.failed |= result.is_err();
        result
    }
    fn suppress_inner(&mut self) -> Result<(), Error> {
        let a = self.authority;
        a.check_db(self.db)?;
        self.suppressed += self.db.execute(
            "UPDATE owner_operator_graph_outbox SET state='suppressed' WHERE registry_id=?1 AND profile=?2 AND owner_mxid=?3 AND scope_id=?4 AND graph_id=?5 AND state='pending_binding'",
            params![a.registry.0.id,a.profile.digest,a.profile.account,self.scope,self.graph],
        )? as u64;
        a.check_db(self.db)
    }
}

impl OwnerResourceRegistry {
    /// Recover an exact original result before reobserving a remote recipient.
    /// A saved success remains historical success even if that target later
    /// retires. A different scope, payload or CAS conflicts instead of replaying.
    pub async fn operator_graph_command_receipt(
        &self,
        profile: &ProfileAuthority,
        command: GraphJournalCommand,
    ) -> Result<Option<GraphJournalReceipt>, Error> {
        validate_command(&command)?;
        let a = Authority {
            registry: self.clone(),
            profile: profile.clone(),
        };
        a.check()?;
        let after = a.clone();
        let result = self
            .0
            .store
            .call(8192, move |db| {
                a.check_db(&db.db)?;
                let receipt = read_receipt(&db.db, &a, &command)?;
                a.check_db(&db.db)?;
                Ok(receipt)
            })
            .await?;
        after.check()?;
        Ok(result)
    }

    pub async fn operator_graph_record_page(
        &self,
        profile: &ProfileAuthority,
        scope: &str,
        after: Option<String>,
    ) -> Result<GraphJournalRecordPage, Error> {
        exact_id(scope)?;
        if let Some(after) = &after {
            exact_id(after)?;
        }
        let a = Authority {
            registry: self.clone(),
            profile: profile.clone(),
        };
        a.check()?;
        let final_authority = a.clone();
        let scope = scope.to_owned();
        let result = self.0.store.call(32 * 1024, move |db| {
            a.check_db(&db.db)?;
            let mut items = db.db.prepare("SELECT CASE WHEN length(graph_id)<=255 THEN graph_id END,revision,length(document) FROM owner_operator_graphs WHERE registry_id=?1 AND profile=?2 AND owner_mxid=?3 AND scope_id=?4 AND (?5 IS NULL OR graph_id>?5) ORDER BY graph_id LIMIT 17")?
                .query_map(params![a.registry.0.id,a.profile.digest,a.profile.account,scope,after], |r| Ok(GraphJournalRecordIndex { graph_id:r.get(0)?,revision:r.get(1)?,document_bytes:r.get(2)? }))?
                .collect::<Result<Vec<_>,_>>()?;
            for item in &items {
                exact_id(&item.graph_id)?;
                if item.revision == 0 || item.document_bytes > MAX_DOCUMENT as u64 { return Err(Error::Quarantined); }
            }
            let next = if items.len() > 16 { items.truncate(16);items.last().map(|item|item.graph_id.clone()) } else {None};
            a.check_db(&db.db)?;
            Ok(GraphJournalRecordPage {items,next})
        }).await?;
        final_authority.check()?;
        Ok(result)
    }
    pub async fn transact_operator_graph(
        &self,
        profile: &ProfileAuthority,
        command: GraphJournalCommand,
        transition: impl FnOnce(
            Option<GraphJournalRecord>,
            &mut GraphJournalWriter<'_>,
        ) -> Result<Vec<u8>, Error>
        + Send
        + 'static,
    ) -> Result<GraphJournalReceipt, Error> {
        validate_command(&command)?;
        let a = Authority {
            registry: self.clone(),
            profile: profile.clone(),
        };
        a.check()?;
        let after = a.clone();
        // Reserve the complete queue budget: an existing and replacement graph
        // can each be four MiB. Message payloads are built and inserted one at a
        // time on this writer, never accumulated in an asynchronous queue.
        let result = self.0.store.call(8 * 1024 * 1024, move |db| {
            a.check_db(&db.db)?;
            let tx = db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            a.check_db(&tx)?;
            // Replay deliberately precedes current revision lookup. Its
            // historical receipt must remain usable after subsequent commands.
            if let Some(receipt) = read_receipt(&tx, &a, &command)? {
                a.check_db(&tx)?;
                tx.commit()?;
                return Ok(receipt);
            }
            let previous = read_record(&tx, &a, &command.scope_id, &command.graph_id)?;
            if previous.as_ref().map_or(0, |r| r.revision) != command.expected_revision {
                return Err(Error::Conflict);
            }
            let receipts: u64 = tx.query_row("SELECT COUNT(*) FROM owner_operator_graph_commands", [], |r| r.get(0))?;
            if receipts >= 100_000 { return Err(Error::Capacity); }
            if previous.is_none() {
                let graphs: u64 = tx.query_row("SELECT COUNT(*) FROM owner_operator_graphs", [], |r| r.get(0))?;
                if graphs >= 10_000 { return Err(Error::Capacity); }
            }
            let mut writer = GraphJournalWriter { db: &tx, authority: &a, scope: &command.scope_id, graph: &command.graph_id, revision: command.expected_revision + 1, queued: 0, suppressed: 0, failed: false };
            a.check_db(&tx)?;
            let changed = transition(previous, &mut writer);
            a.check_db(&tx)?;
            let document = changed?;
            if writer.failed { return Err(Error::Unqualified); }
            if document.len() > MAX_DOCUMENT { return Err(Error::Capacity); }
            let receipt = GraphJournalReceipt { command_id: command.command_id.clone(), scope_id: command.scope_id.clone(), graph_id: command.graph_id.clone(), revision: command.expected_revision + 1, document_digest: bytes_digest(&document), queued_messages: writer.queued, suppressed_messages: writer.suppressed };
            tx.execute(
                "INSERT INTO owner_operator_graphs VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(registry_id,profile,owner_mxid,scope_id,graph_id) DO UPDATE SET revision=excluded.revision,document=excluded.document,document_digest=excluded.document_digest",
                params![a.registry.0.id,a.profile.digest,a.profile.account,command.scope_id,command.graph_id,receipt.revision,document,receipt.document_digest],
            )?;
            tx.execute(
                "INSERT INTO owner_operator_graph_commands VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![a.profile.digest,command.command_id,a.registry.0.id,a.profile.account,command.scope_id,command.graph_id,command.expected_revision,command.input_digest,request_digest(&a, &command)?,serde_json::to_string(&receipt)?],
            )?;
            a.check_db(&tx)?;
            tx.commit()?;
            Ok(receipt)
        }).await?;
        after.check().map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }

    /// Native-only scoped read; raw document bytes are not a mini-app service.
    pub async fn operator_graph_record(
        &self,
        profile: &ProfileAuthority,
        scope: &str,
        graph: &str,
    ) -> Result<Option<GraphJournalRecord>, Error> {
        exact_id(scope)?;
        exact_id(graph)?;
        let a = Authority {
            registry: self.clone(),
            profile: profile.clone(),
        };
        a.check()?;
        let after = a.clone();
        let (scope, graph) = (scope.to_owned(), graph.to_owned());
        let result = self
            .0
            .store
            .call(MAX_DOCUMENT as u32 + 4096, move |db| {
                a.check_db(&db.db)?;
                let record = read_record(&db.db, &a, &scope, &graph)?;
                a.check_db(&db.db)?;
                Ok(record)
            })
            .await?;
        after.check()?;
        Ok(result)
    }
}

fn validate_command(command: &GraphJournalCommand) -> Result<(), Error> {
    identifier(&command.command_id, 128)?;
    exact_id(&command.scope_id)?;
    exact_id(&command.graph_id)?;
    hash(&command.input_digest)?;
    if command.expected_revision >= i64::MAX as u64 {
        return Err(Error::Capacity);
    }
    Ok(())
}
fn request_digest(a: &Authority, command: &GraphJournalCommand) -> Result<String, Error> {
    Ok(canonical::digest(
        &json!({"kind":"operator_graph_command_v1","registry":a.registry.0.id,"profile":a.profile.digest,"owner":a.profile.account,"command":command.command_id,"scope":command.scope_id,"graph":command.graph_id,"expectedRevision":command.expected_revision,"inputDigest":command.input_digest}),
    )?)
}
fn read_receipt(
    db: &Connection,
    a: &Authority,
    command: &GraphJournalCommand,
) -> Result<Option<GraphJournalReceipt>, Error> {
    let saved: Option<(String,String)> = db.query_row(
        "SELECT request_digest,CASE WHEN length(CAST(receipt AS BLOB))<=4096 THEN receipt END FROM owner_operator_graph_commands WHERE profile=?1 AND command_id=?2",
        params![a.profile.digest,command.command_id], |r| Ok((r.get(0)?,r.get(1)?)),
    ).optional()?;
    saved
        .map(|(digest, saved)| {
            if digest != request_digest(a, command)? {
                return Err(Error::Conflict);
            }
            let receipt: GraphJournalReceipt = serde_json::from_str(&saved)?;
            if receipt.command_id != command.command_id
                || receipt.scope_id != command.scope_id
                || receipt.graph_id != command.graph_id
                || receipt.revision != command.expected_revision + 1
                || receipt.queued_messages > MAX_MESSAGES
                || receipt.suppressed_messages > MAX_MESSAGES
            {
                return Err(Error::Quarantined);
            }
            hash(&receipt.document_digest)?;
            Ok(receipt)
        })
        .transpose()
}

fn exact_id(id: &str) -> Result<(), Error> {
    if id.is_empty()
        || id.chars().count() > 255
        || id.trim() != id
        || id.chars().any(char::is_control)
    {
        return Err(Error::Unqualified);
    }
    Ok(())
}
fn read_record(
    db: &Connection,
    a: &Authority,
    scope: &str,
    graph: &str,
) -> Result<Option<GraphJournalRecord>, Error> {
    let row: Option<(u64,Vec<u8>,String)> = db.query_row(
        "SELECT revision,CASE WHEN length(document)<=4194304 THEN document END,document_digest FROM owner_operator_graphs WHERE registry_id=?1 AND profile=?2 AND owner_mxid=?3 AND scope_id=?4 AND graph_id=?5",
        params![a.registry.0.id,a.profile.digest,a.profile.account,scope,graph],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).optional()?;
    row.map(|(revision, document, digest)| {
        if revision == 0 || bytes_digest(&document) != digest {
            return Err(Error::Quarantined);
        }
        Ok(GraphJournalRecord { revision, document })
    })
    .transpose()
}
