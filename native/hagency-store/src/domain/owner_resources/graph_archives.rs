//! Private, resumable copies of original operator documents. This is history
//! custody, not active graph adoption. No runtime tables or messages are written.
//! A host retains the exact reviewed source and installation exclusion through
//! every write; serialized counts or an archive receipt grant no such authority.
mod reads;
use super::*;
use hagency_core::project::identifier;
use sha2::{Digest, Sha256};

pub const GRAPH_ARCHIVE_CHUNK_BYTES: usize = 1024 * 1024;
const DOCUMENTS: [(&str, u64); 3] = [
    ("task_graphs.json", 4 * 1024 * 1024),
    ("messages.json", 64 * 1024 * 1024),
    (".msg_counter", 512),
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorGraphHistorySummary {
    pub source_digest: String,
    pub present_documents: usize,
    pub graphs: usize,
    pub nodes: usize,
    pub messages: usize,
    pub dispatch_messages: usize,
    pub orphan_dispatch_messages: usize,
    pub unresolved_principal_names: usize,
    pub original_counter: Option<String>,
    pub effective_counter: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    version: u8,
    history: OperatorGraphHistorySummary,
    lengths: [Option<u64>; 3],
}
impl Manifest {
    fn validate(&self) -> Result<(), Error> {
        hash(&self.history.source_digest)?;
        let h = &self.history;
        let counter = |s: &str| s.parse::<u64>().ok().filter(|n| n.to_string() == s);
        if self.version != 1
            || h.present_documents != self.lengths.iter().flatten().count()
            || h.graphs > 10_000
            || h.nodes > 10_000
            || h.messages > 10_000
            || h.dispatch_messages > h.messages
            || h.orphan_dispatch_messages > h.dispatch_messages
            || h.unresolved_principal_names > h.graphs + h.nodes
            || counter(&h.effective_counter).is_none()
            || h.original_counter
                .as_deref()
                .is_some_and(|s| counter(s).is_none())
            || h.original_counter.is_some() != self.lengths[2].is_some()
            || self
                .lengths
                .iter()
                .zip(DOCUMENTS)
                .any(|(len, (_, max))| len.is_some_and(|n| n > max))
        {
            return Err(Error::Unqualified);
        }
        Ok(())
    }
    fn chunk_length(&self, document: usize, index: usize) -> Result<usize, Error> {
        let length = self
            .lengths
            .get(document)
            .and_then(|v| *v)
            .ok_or(Error::Unqualified)?;
        let offset = index
            .checked_mul(GRAPH_ARCHIVE_CHUNK_BYTES)
            .ok_or(Error::Capacity)? as u64;
        if offset >= length {
            return Err(Error::Unqualified);
        }
        Ok((length - offset).min(GRAPH_ARCHIVE_CHUNK_BYTES as u64) as usize)
    }
}

/// No document contents, principal names, message tokens or source paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorGraphArchiveReceipt {
    pub command_id: String,
    pub history: OperatorGraphHistorySummary,
    pub state: String,
    pub runtime_delivery_authorized: bool,
    pub source_writer_fenced: bool,
}

/// Native-only and finite. The caller's gate retains the reviewed files and
/// installation exclusion, and revalidates them on the actual writer thread.
/// This type is neither serializable nor constructible by a mini-app payload.
pub struct PreparedOperatorGraphArchive {
    registry: OwnerResourceRegistry,
    profile: ProfileAuthority,
    command_id: String,
    source_binding: String,
    manifest: Manifest,
    expires: Instant,
    source: Arc<dyn Fn() -> bool + Send + Sync>,
    source_contents: Arc<dyn Fn() -> bool + Send + Sync>,
}
#[derive(Clone)]
struct Authority {
    registry: OwnerResourceRegistry,
    profile: ProfileAuthority,
    expires: Instant,
    source: Arc<dyn Fn() -> bool + Send + Sync>,
    source_contents: Arc<dyn Fn() -> bool + Send + Sync>,
}
impl Authority {
    fn check_contents(&self) -> Result<(), Error> {
        self.check()?;
        if !(self.source_contents)() { return Err(Error::LocalAuthority); }
        self.check()
    }
    fn check(&self) -> Result<(), Error> {
        self.profile.check()?;
        if self.registry.0.closed.load(Ordering::Acquire)
            || Instant::now() >= self.expires
            || !(self.source)()
        {
            return Err(Error::LocalAuthority);
        }
        Ok(())
    }
}
impl PreparedOperatorGraphArchive {
    fn authority(&self) -> Authority {
        Authority {
            registry: self.registry.clone(),
            profile: self.profile.clone(),
            expires: self.expires,
            source: self.source.clone(),
            source_contents: self.source_contents.clone(),
        }
    }
    /// Recording the manifest is resumable custody, never a completed import.
    pub async fn begin(&self) -> Result<OperatorGraphArchiveReceipt, Error> {
        let a = self.authority();
        a.check()?;
        let after = a.clone();
        let id = self.command_id.clone();
        let binding = self.source_binding.clone();
        let manifest = self.manifest.clone();
        let result=self.registry.0.store.call(4096, move |db| {
            a.check()?;
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            registry_identity(&tx,&a.registry.0.id)?;
            let existing=read(&tx,&id)?;
            let state=if let Some(saved)=existing {
                exact(&saved,&a,&binding,&manifest)?; saved.state
            } else {
                let collision:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM owner_operator_graph_archives WHERE profile=?1 AND source_binding=?2)",params![a.profile.digest,binding],|r|r.get(0))?;
                if collision {return Err(Error::Conflict);}
                let count:u64=tx.query_row("SELECT COUNT(*) FROM owner_operator_graph_archives",[],|r|r.get(0))?;
                if count>=16 {return Err(Error::Capacity);}
                tx.execute("INSERT INTO owner_operator_graph_archives VALUES(?1,?2,?3,?4,?5,?6,'copying')",params![id,a.registry.0.id,a.profile.digest,a.profile.account,binding,serde_json::to_string(&manifest)?])?;
                "copying".into()
            };
            a.check()?; tx.commit()?;
            Ok(receipt(id,manifest,state))
        }).await?;
        after.check().map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
    /// At most one MiB is admitted to the bounded domain writer queue. A partial
    /// copy remains invisible to readers; replay compares original bytes.
    pub async fn write_chunk(
        &self,
        document: usize,
        index: usize,
        bytes: Vec<u8>,
    ) -> Result<(), Error> {
        if bytes.len() != self.manifest.chunk_length(document, index)? {
            return Err(Error::Unqualified);
        }
        let a = self.authority();
        a.check()?;
        let after = a.clone();
        let id = self.command_id.clone();
        let binding = self.source_binding.clone();
        let manifest = self.manifest.clone();
        self.registry.0.store.call((bytes.len()+4096) as u32,move |db| {
            a.check()?;
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let saved=read(&tx,&id)?.ok_or(Error::NotFound)?;
            exact(&saved,&a,&binding,&manifest)?;
            let old:Option<Vec<u8>>=tx.query_row("SELECT bytes FROM owner_operator_graph_archive_chunks WHERE command_id=?1 AND document=?2 AND chunk_index=?3",params![id,document,index],|r|r.get(0)).optional()?;
            if let Some(old)=old {if old!=bytes {return Err(Error::Conflict);}}
            else {
                if saved.state!="copying" {return Err(Error::Quarantined);}
                tx.execute("INSERT INTO owner_operator_graph_archive_chunks VALUES(?1,?2,?3,?4)",params![id,document,index,bytes])?;
            }
            a.check()?; tx.commit()?; Ok(())
        }).await?;
        after.check().map_err(|_| Error::OutcomeUnknown)
    }
    /// Rehash every original byte under the same transaction that publishes
    /// the completed archive. No graph dispatch, resource or message is created.
    pub async fn seal(&self) -> Result<OperatorGraphArchiveReceipt, Error> {
        let a = self.authority();
        a.check()?;
        let after = a.clone();
        let id = self.command_id.clone();
        let binding = self.source_binding.clone();
        let manifest = self.manifest.clone();
        let result=self.registry.0.store.call((GRAPH_ARCHIVE_CHUNK_BYTES+4096) as u32,move |db| {
            a.check()?;
            let tx=db.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let saved=read(&tx,&id)?.ok_or(Error::NotFound)?;
            exact(&saved,&a,&binding,&manifest)?;
            verify_chunks(&tx,&id,&manifest,&a)?;
            a.check_contents()?;
            if saved.state=="copying" {tx.execute("UPDATE owner_operator_graph_archives SET state='archived' WHERE command_id=?1",[&id])?;}
            a.check()?; tx.commit()?;
            Ok(receipt(id,manifest,"archived".into()))
        }).await?;
        after.check().map_err(|_| Error::OutcomeUnknown)?;
        Ok(result)
    }
}

impl OwnerResourceRegistry {
    /// Trusted host only. `history` must come from the native document parser;
    /// `source` retains/checks identity and exclusion on each chunk. The
    /// independent `source_contents` rechecks all bytes immediately before the
    /// sealed commit. Copying never exposes partial data, so whole-file reads
    /// need not be repeated for every one-MiB chunk.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_operator_graph_archive(
        &self,
        profile: ProfileAuthority,
        command_id: String,
        source_binding: String,
        history: OperatorGraphHistorySummary,
        lengths: [Option<u64>; 3],
        expires: Instant,
        source: impl Fn() -> bool + Send + Sync + 'static,
        source_contents: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<PreparedOperatorGraphArchive, Error> {
        identifier(&command_id, 128)?;
        hash(&source_binding)?;
        if expires > Instant::now() + Duration::from_secs(60) {
            return Err(Error::LocalAuthority);
        }
        let manifest = Manifest {
            version: 1,
            history,
            lengths,
        };
        manifest.validate()?;
        let p = PreparedOperatorGraphArchive {
            registry: self.clone(),
            profile,
            command_id,
            source_binding,
            manifest,
            expires,
            source: Arc::new(source),
            source_contents: Arc::new(source_contents),
        };
        p.authority().check()?;
        Ok(p)
    }
    /// Recovery needs fresh native profile authority, not old source custody.
    /// An incomplete command is reported as copying, never implicitly resumed.
    pub async fn operator_graph_archive_receipt(
        &self,
        profile: &ProfileAuthority,
        command_id: String,
    ) -> Result<Option<OperatorGraphArchiveReceipt>, Error> {
        profile.check()?;
        identifier(&command_id, 128)?;
        let p = profile.clone();
        let registry = self.clone();
        let result = self
            .0
            .store
            .call(4096, move |db| {
                p.check()?;
                if registry.0.closed.load(Ordering::Acquire) {
                    return Err(Error::LocalAuthority);
                }
                let Some(saved) = read(&db.db, &command_id)? else {
                    return Ok(None);
                };
                visible(&saved, &p, &registry.0.id)?;
                p.check()?;
                Ok(Some(receipt(command_id, saved.manifest, saved.state)))
            })
            .await?;
        profile.check()?;
        Ok(result)
    }
    /// Private native history reader. No script route exposes raw documents.
    /// It deliberately refuses every chunk until the whole archive is sealed.
    pub async fn operator_graph_archive_chunk(
        &self,
        profile: &ProfileAuthority,
        command_id: String,
        document: usize,
        index: usize,
    ) -> Result<Vec<u8>, Error> {
        profile.check()?;
        identifier(&command_id, 128)?;
        let p = profile.clone();
        let registry = self.clone();
        let result=self.0.store.call((GRAPH_ARCHIVE_CHUNK_BYTES+4096) as u32,move |db| {
            p.check()?; if registry.0.closed.load(Ordering::Acquire) {return Err(Error::LocalAuthority);}
            let saved=read(&db.db,&command_id)?.ok_or(Error::NotFound)?;
            visible(&saved,&p,&registry.0.id)?;
            if saved.state!="archived" {return Err(Error::Unqualified);}
            let length=saved.manifest.chunk_length(document,index)?;
            let bytes:Vec<u8>=db.db.query_row("SELECT CASE WHEN length(bytes)<=1048576 THEN bytes END FROM owner_operator_graph_archive_chunks WHERE command_id=?1 AND document=?2 AND chunk_index=?3",params![command_id,document,index],|r|r.get(0))?;
            if bytes.len()!=length {return Err(Error::Quarantined);}
            p.check()?; Ok(bytes)
        }).await?;
        profile.check()?;
        Ok(result)
    }
}
struct Saved {
    registry: String,
    profile: String,
    owner: String,
    binding: String,
    manifest: Manifest,
    state: String,
}
fn read(db: &Connection, id: &str) -> Result<Option<Saved>, Error> {
    let row:Option<(String,String,String,String,String,String)>=db.query_row("SELECT registry_id,profile,owner_mxid,source_binding,CASE WHEN length(CAST(manifest AS BLOB))<=4096 THEN manifest END,state FROM owner_operator_graph_archives WHERE command_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
    row.map(|(registry, profile, owner, binding, raw, state)| {
        let manifest: Manifest = serde_json::from_str(&raw)?;
        manifest.validate()?;
        if !matches!(state.as_str(), "copying" | "archived") {
            return Err(Error::Quarantined);
        }
        Ok(Saved {
            registry,
            profile,
            owner,
            binding,
            manifest,
            state,
        })
    })
    .transpose()
}
fn registry_identity(db: &Connection, id: &str) -> Result<(), Error> {
    let stored: String = db.query_row(
        "SELECT id FROM owner_resource_identity WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    if stored != id {
        return Err(Error::LocalAuthority);
    }
    Ok(())
}
fn visible(saved: &Saved, p: &ProfileAuthority, registry: &str) -> Result<(), Error> {
    if saved.registry != registry || saved.profile != p.digest || saved.owner != p.account {
        return Err(Error::LocalAuthority);
    }
    Ok(())
}
fn exact(saved: &Saved, a: &Authority, binding: &str, manifest: &Manifest) -> Result<(), Error> {
    visible(saved, &a.profile, &a.registry.0.id)?;
    if saved.binding != binding || saved.manifest != *manifest {
        return Err(Error::Conflict);
    }
    Ok(())
}
fn receipt(id: String, manifest: Manifest, state: String) -> OperatorGraphArchiveReceipt {
    OperatorGraphArchiveReceipt {
        command_id: id,
        history: manifest.history,
        state,
        runtime_delivery_authorized: false,
        source_writer_fenced: false,
    }
}
fn verify_chunks(db: &Connection, id: &str, m: &Manifest, a: &Authority) -> Result<(), Error> {
    let mut digest = Sha256::new();
    digest.update(b"hagency.operator-graph-documents.v1\0");
    let mut count = 0;
    for (document, ((name, _), length)) in DOCUMENTS.iter().zip(m.lengths).enumerate() {
        digest.update(name.as_bytes());
        digest.update([0, u8::from(length.is_some())]);
        if let Some(length) = length {
            digest.update(length.to_be_bytes());
            for index in 0..(length as usize).div_ceil(GRAPH_ARCHIVE_CHUNK_BYTES) {
                // Revocation/deadline remains effective while reading large history.
                a.profile.check()?;
                if Instant::now() >= a.expires {
                    return Err(Error::LocalAuthority);
                }
                let bytes:Option<Vec<u8>>=db.query_row("SELECT CASE WHEN length(bytes)<=1048576 THEN bytes END FROM owner_operator_graph_archive_chunks WHERE command_id=?1 AND document=?2 AND chunk_index=?3",params![id,document,index],|r|r.get(0)).optional()?;
                let bytes = bytes.ok_or(Error::Unqualified)?;
                if bytes.len() != m.chunk_length(document, index)? {
                    return Err(Error::Quarantined);
                }
                digest.update(bytes);
                count += 1;
            }
        }
    }
    let actual: u64 = db.query_row(
        "SELECT COUNT(*) FROM owner_operator_graph_archive_chunks WHERE command_id=?1",
        [id],
        |r| r.get(0),
    )?;
    if actual != count || format!("{:x}", digest.finalize()) != m.history.source_digest {
        return Err(Error::Quarantined);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
