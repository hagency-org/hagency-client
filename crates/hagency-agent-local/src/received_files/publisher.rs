use super::{MAX_BYTES, MAX_HELD, MAX_RECORDS, Receipt};
use crate::{
    Error, Ledger, Result, Scope, ToolProposal, attachments::Attachment, inbox::State, json,
};
use rusqlite::{OptionalExtension, TransactionBehavior};
use rustix::fs::{self, Mode, OFlags};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Component, Path},
};
pub struct Prepared {
    proposal: ToolProposal,
    source: Attachment,
    execution: String,
    event: String,
    id: String,
    path: String,
}
impl Prepared {
    pub fn proposal(&self) -> &ToolProposal {
        &self.proposal
    }
}
/// Only the first committed reservation creates this non-Clone owner.
pub struct Custody {
    prepared: Box<Prepared>,
}
pub enum Begin {
    Fresh(Custody),
    Ready(Box<Receipt>),
}
/// Retains the actual workspace directory object. Opening this creates no file.
pub struct Receiver {
    root: File,
    canonical: String,
}
fn io() -> Error {
    Error::Invalid("received file IO or boundary")
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn dir_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK
}
fn directory(parent: &File, name: &std::ffi::OsStr) -> Result<File> {
    fs::openat(parent, name, dir_flags(), Mode::empty())
        .map(File::from)
        .map_err(|_| io())
}
fn root(path: &Path) -> Result<File> {
    if !path.is_absolute() || path.canonicalize().ok().as_deref() != Some(path) {
        return Err(io());
    }
    let mut dir = File::from(fs::open("/", dir_flags(), Mode::empty()).map_err(|_| io())?);
    for part in path.components() {
        match part {
            Component::RootDir => {}
            Component::Normal(name) => dir = directory(&dir, name)?,
            _ => return Err(io()),
        }
    }
    Ok(dir)
}
fn private(dir: &File) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let m = dir.metadata().map_err(|_| io())?;
    if !m.is_dir() || m.mode() & 0o077 != 0 || m.uid() != rustix::process::geteuid().as_raw() {
        return Err(io());
    }
    Ok(())
}
fn child(parent: &File, name: &str, create: bool) -> Result<File> {
    if create {
        match fs::mkdirat(parent, name, Mode::RUSR | Mode::WUSR | Mode::XUSR) {
            Ok(()) => parent.sync_all().map_err(|_| Error::Unknown)?,
            Err(rustix::io::Errno::EXIST) => {}
            Err(_) => return Err(io()),
        }
    }
    let dir = directory(parent, name.as_ref())?;
    private(&dir)?;
    Ok(dir)
}
fn identity(file: &File) -> Result<(u64, u64, i64, i64, i64, i64)> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata().map_err(|_| io())?;
    if !m.is_file()
        || m.nlink() != 1
        || m.mode() & 0o077 != 0
        || m.uid() != rustix::process::geteuid().as_raw()
    {
        return Err(io());
    }
    Ok((
        m.dev(),
        m.ino(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    ))
}
fn read_file(parent: &File, name: &str) -> Result<File> {
    let file = File::from(
        fs::openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|_| io())?,
    );
    identity(&file)?;
    Ok(file)
}
fn running(ledger: &Ledger, p: &Prepared) -> Result<()> {
    let record = ledger.inbox_record(&ledger.owner, &p.proposal.dispatch)?;
    if record.state != State::Running
        || record.execution_id.as_deref() != Some(&p.execution)
        || record.dispatch.scope() != p.proposal.scope
        || record.dispatch.event_id != p.event
    {
        return Err(Error::Unauthorized);
    }
    Ok(())
}
impl Receiver {
    pub fn canonical_directory(&self) -> &Path {
        Path::new(&self.canonical)
    }
    pub fn revalidate(&self, receipt: &Receipt) -> Result<()> {
        self.verify(receipt)
    }

    pub fn open(workspace: &Path) -> Result<Self> {
        Ok(Self {
            root: root(workspace)?,
            canonical: workspace.to_str().ok_or_else(io)?.into(),
        })
    }
    fn current_root(&self) -> Result<()> {
        let current = root(Path::new(&self.canonical))?;
        if !hagency_platform::same_directory(&self.root, &current).map_err(|_| io())? {
            return Err(io());
        }
        Ok(())
    }
    fn parent(&self, path: &str, create: bool) -> Result<(File, String)> {
        self.current_root()?;
        let pieces: Vec<_> = path.split('/').collect();
        if pieces.len() != 3
            || pieces[0] != ".hagency-received"
            || !pieces[1].starts_with("binding_")
            || !pieces[2].starts_with("attachment_")
            || pieces
                .iter()
                .any(|s| s.contains(['\\', '\0']) || s.contains(".."))
        {
            return Err(io());
        }
        let base = child(&self.root, pieces[0], create)?;
        let binding = child(&base, pieces[1], create)?;
        Ok((binding, pieces[2].into()))
    }
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &self,
        ledger: &Ledger,
        scope: &Scope,
        dispatch: &str,
        execution: &str,
        event: &str,
        source: Attachment,
        thread: &str,
        turn: &str,
        call: &str,
        now: i64,
    ) -> Result<Prepared> {
        self.current_root()?;
        source.validate()?;
        if source.declared_size.is_some_and(|s| s > MAX_BYTES as u64)
            || !(0..=i64::MAX - 300).contains(&now)
        {
            return Err(Error::Invalid("received file capacity/time"));
        }
        for v in [dispatch, execution, event, thread, turn, call] {
            crate::key(v)?;
        }
        let id = digest(
            json(&(
                scope,
                dispatch,
                execution,
                event,
                &source.descriptor_digest,
                &self.canonical,
            ))?
            .as_bytes(),
        );
        let binding =
            digest(json(&(&ledger.owner, &scope.agent, &scope.binding, &scope.room))?.as_bytes());
        let path = format!(".hagency-received/binding_{binding}/attachment_{id}.bin");
        let proposal = ToolProposal {
            scope: scope.clone(),
            dispatch: dispatch.into(),
            tool: "matrix.receive_file".into(),
            arguments: serde_json::json!({"codexThreadId":thread,"turnId":turn,"callId":call,"eventId":event,"attachment":source.summary()?,"relativePath":path}),
            canonical_directory: self.canonical.clone(),
            risk: "high".into(),
            policy_revision: ledger.policy_snapshot(scope)?.map(|v| v.revision),
            expires: now + 300,
        };
        let prepared = Prepared {
            proposal,
            source,
            execution: execution.into(),
            event: event.into(),
            id,
            path,
        };
        running(ledger, &prepared)?;
        Ok(prepared)
    }
    /// Call only after fresh host authorization; local exact approval is consumed
    /// here before reservation. A duplicate never regains download/write custody.
    pub fn begin(&self, ledger: &mut Ledger, prepared: Prepared, now: i64) -> Result<Begin> {
        self.current_root()?;
        running(ledger, &prepared)?;
        ledger.authorize_tool(&prepared.proposal, now)?;
        let tx = ledger
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prior: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT state,receipt FROM received_files WHERE id=?",
                [&prepared.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((state, receipt)) = prior {
            if state != "ready" {
                return Err(Error::Unknown);
            }
            let receipt: Receipt = serde_json::from_str(&receipt.ok_or(Error::Unknown)?)?;
            if receipt.relative_path != prepared.path {
                return Err(Error::Conflict);
            }
            self.verify(&receipt)?;
            tx.commit()?;
            return Ok(Begin::Ready(Box::new(receipt)));
        }
        let (count,held):(u64,u64)=tx.query_row("SELECT count(*),coalesce(sum(CASE WHEN state='ready' THEN json_extract(receipt,'$.bytes') WHEN state='failed' THEN 0 ELSE 8388608 END),0) FROM received_files",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
        if count >= MAX_RECORDS || held.saturating_add(MAX_BYTES as u64) > MAX_HELD {
            return Err(Error::Invalid("received file quota"));
        }
        tx.execute("INSERT INTO received_files(id,scope,dispatch,execution,event,descriptor_digest,workspace,relative_path,state) VALUES (?,?,?,?,?,?,?,?,'reserved')",rusqlite::params![prepared.id,json(&prepared.proposal.scope)?,prepared.proposal.dispatch,prepared.execution,prepared.event,prepared.source.descriptor_digest,self.canonical,prepared.path])?;
        tx.commit()?;
        Ok(Begin::Fresh(Custody {
            prepared: Box::new(prepared),
        }))
    }
    /// Known failure before any filesystem write releases its byte reservation.
    pub fn fail_before_write(&self, ledger: &mut Ledger, custody: Custody) -> Result<()> {
        if ledger.db.execute(
            "UPDATE received_files SET state='failed' WHERE id=? AND state='reserved'",
            [custody.prepared.id],
        )? != 1
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    /// Caller rechecks Server/SDK authority after download and immediately before
    /// this synchronous operation. Exact local policy is checked once more.
    pub fn publish(
        &self,
        ledger: &mut Ledger,
        custody: Custody,
        bytes: &[u8],
        now: i64,
    ) -> Result<Receipt> {
        let p = custody.prepared;
        let checked = (|| -> Result<()> {
            self.current_root()?;
            running(ledger, &p)?;
            ledger.tool_disposition(&p.proposal, now)?;
            if bytes.len() > MAX_BYTES
                || p.source
                    .declared_size
                    .is_some_and(|s| s != bytes.len() as u64)
            {
                return Err(Error::Invalid("received byte length"));
            }
            Ok(())
        })();
        if let Err(error) = checked {
            if ledger.db.execute(
                "UPDATE received_files SET state='failed' WHERE id=? AND state='reserved'",
                [&p.id],
            )? != 1
            {
                return Err(Error::Unknown);
            }
            return Err(error);
        }
        // Commit the irreversible boundary before mkdir, staging, rename or sync.
        if ledger.db.execute(
            "UPDATE received_files SET state='write_possible' WHERE id=? AND state='reserved'",
            [&p.id],
        )? != 1
        {
            return Err(Error::Unknown);
        }
        let receipt = (|| -> Result<Receipt> {
            let (parent, name) = self.parent(&p.path, true)?;
            let staging = format!(".stage-{}", p.id);
            let mut file = File::from(
                fs::openat(
                    &parent,
                    &staging,
                    OFlags::RDWR
                        | OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC
                        | OFlags::NONBLOCK,
                    Mode::RUSR | Mode::WUSR,
                )
                .map_err(|_| Error::Unknown)?,
            );
            identity(&file)?;
            file.write_all(bytes).map_err(|_| Error::Unknown)?;
            file.sync_all().map_err(|_| Error::Unknown)?;
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            fs::renameat_with(
                &parent,
                &staging,
                &parent,
                &name,
                fs::RenameFlags::NOREPLACE,
            )
            .map_err(|_| Error::Unknown)?;
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            return Err(Error::Unknown);
            parent.sync_all().map_err(|_| Error::Unknown)?;
            let published = read_file(&parent, &name)?;
            if !hagency_platform::same_file(&file, &published).map_err(|_| io())? {
                return Err(Error::Unknown);
            }
            let receipt = Receipt {
                relative_path: p.path.clone(),
                bytes: bytes.len() as u64,
                sha256: digest(bytes),
                identity: identity(&published)?,
                workspace_identity: hagency_platform::directory_identity(&self.root)
                    .map_err(|_| io())?,
                parent_identity: hagency_platform::directory_identity(&parent).map_err(|_| io())?,
            };
            self.verify(&receipt)?;
            Ok(receipt)
        })()
        .map_err(|_| Error::Unknown)?;
        if ledger.db.execute("UPDATE received_files SET state='ready',receipt=? WHERE id=? AND state='write_possible'",(json(&receipt)?,&p.id))?!=1 {return Err(Error::Unknown)}
        Ok(receipt)
    }
    fn verify(&self, receipt: &Receipt) -> Result<()> {
        let (parent, name) = self.parent(&receipt.relative_path, false)?;
        if hagency_platform::directory_identity(&self.root).map_err(|_| io())?
            != receipt.workspace_identity
            || hagency_platform::directory_identity(&parent).map_err(|_| io())?
                != receipt.parent_identity
        {
            return Err(Error::Unknown);
        }
        let mut file = read_file(&parent, &name)?;
        let before = identity(&file)?;
        if before != receipt.identity || receipt.bytes > MAX_BYTES as u64 {
            return Err(Error::Unknown);
        }
        let mut data = Vec::new();
        (&mut file)
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut data)
            .map_err(|_| io())?;
        let again = read_file(&parent, &name)?;
        if data.len() as u64 != receipt.bytes
            || digest(&data) != receipt.sha256
            || identity(&file)? != before
            || !hagency_platform::same_file(&file, &again).map_err(|_| io())?
        {
            return Err(Error::Unknown);
        }
        self.current_root()?;
        Ok(())
    }
}
