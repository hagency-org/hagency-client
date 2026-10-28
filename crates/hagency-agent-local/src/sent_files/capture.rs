use super::*;
use rustix::fs::{self, Mode, OFlags};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Component, Path},
    sync::{Arc, OnceLock},
};
fn io() -> Error {
    Error::Invalid("outgoing file boundary or IO")
}
fn flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK
}
fn directory(parent: &File, name: &std::ffi::OsStr) -> Result<File> {
    fs::openat(parent, name, flags(), Mode::empty())
        .map(File::from)
        .map_err(|_| io())
}
fn root(path: &Path) -> Result<File> {
    if !path.is_absolute() || path.canonicalize().ok().as_deref() != Some(path) {
        return Err(io());
    }
    let mut file = File::from(fs::open("/", flags(), Mode::empty()).map_err(|_| io())?);
    for part in path.components() {
        match part {
            Component::RootDir => {}
            Component::Normal(name) => file = directory(&file, name)?,
            _ => return Err(io()),
        }
    }
    Ok(file)
}
fn identity(file: &File) -> Result<(u64, u64, u64, i64, i64, i64, i64)> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata().map_err(|_| io())?;
    if !m.is_file()
        || m.nlink() != 1
        || m.uid() != rustix::process::geteuid().as_raw()
        || m.len() > crate::file_delivery::MAX_BYTES as u64
    {
        return Err(io());
    }
    Ok((
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    ))
}
fn read(parent: &File, name: &str) -> Result<File> {
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
pub struct Sender {
    root: File,
    canonical: String,
}
/// Actual frozen bytes and held physical objects. Never Clone/Serialize/Debug.
pub struct Captured {
    pub(super) id: String,
    pub(super) proposal: ToolProposal,
    pub(super) execution: String,
    pub(super) relative_path: String,
    pub(super) manifest: Manifest,
    pub(super) bytes: Vec<u8>,
    parents: Vec<(String, File)>,
    file: File,
    identity: (u64, u64, u64, i64, i64, i64, i64),
    _permit: tokio::sync::OwnedSemaphorePermit,
}
impl Captured {
    pub fn proposal(&self) -> &ToolProposal {
        &self.proposal
    }
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn id(&self) -> &str {
        &self.id
    }
}
impl Sender {
    pub fn open(workspace: &Path) -> Result<Self> {
        Ok(Self {
            root: root(workspace)?,
            canonical: workspace.to_str().ok_or_else(io)?.into(),
        })
    }
    pub fn canonical_directory(&self) -> &Path {
        Path::new(&self.canonical)
    }
    fn current(&self) -> Result<()> {
        if !hagency_platform::same_directory(&self.root, &root(Path::new(&self.canonical))?)
            .map_err(|_| io())?
        {
            return Err(io());
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub fn capture(
        &self,
        ledger: &Ledger,
        scope: &Scope,
        dispatch: &str,
        execution: &str,
        path: &str,
        filename: Option<&str>,
        caption: Option<String>,
        thread: &str,
        turn: &str,
        call: &str,
        now: i64,
    ) -> Result<Captured> {
        running(ledger, scope, dispatch, execution)?;
        self.current()?;
        if !(0..=i64::MAX - 300).contains(&now)
            || path.is_empty()
            || path.len() > 4096
            || path.contains(['\\', '\0'])
            || path.chars().any(char::is_control)
            || path
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == "..")
        {
            return Err(io());
        }
        for key in [dispatch, execution, thread, turn, call] {
            crate::key(key)?;
        }
        let pieces = path.split('/').collect::<Vec<_>>();
        static PERMITS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
        let permit = PERMITS
            .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(4)))
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Invalid("outgoing capture capacity"))?;
        let mut parents = Vec::new();
        let mut parent = &self.root;
        for name in &pieces[..pieces.len() - 1] {
            let dir = directory(parent, name.as_ref())?;
            parents.push(((*name).into(), dir));
            parent = &parents.last().unwrap().1;
        }
        let mut file = read(parent, pieces.last().unwrap())?;
        let before = identity(&file)?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(crate::file_delivery::MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| io())?;
        if bytes.len() as u64 != before.2 || identity(&file)? != before {
            return Err(io());
        }
        let manifest = Manifest {
            filename: filename.unwrap_or(pieces.last().unwrap()).into(),
            caption,
            bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        manifest.validate()?;
        let id = crate::hash(&(
            &ledger.owner,
            scope,
            dispatch,
            execution,
            &self.canonical,
            path,
            &manifest,
        ))?;
        let proposal = ToolProposal {
            scope: scope.clone(),
            dispatch: dispatch.into(),
            tool: "matrix.send_file".into(),
            arguments: serde_json::json!({"codexThreadId":thread,"turnId":turn,"callId":call,"path":path,"file":manifest,"manifestDigest":manifest.digest()?,"deliveryId":id}),
            canonical_directory: self.canonical.clone(),
            risk: "high".into(),
            policy_revision: ledger.policy_snapshot(scope)?.map(|p| p.revision),
            expires: now + 300,
        };
        let capture = Captured {
            id,
            proposal,
            execution: execution.into(),
            relative_path: path.into(),
            manifest,
            bytes,
            parents,
            file,
            identity: before,
            _permit: permit,
        };
        self.verify(&capture)?;
        if ledger
            .sent_file(scope, dispatch, execution, &capture.id)?
            .is_none()
        {
            reject_pending(&ledger.db, &capture)?;
        }
        Ok(capture)
    }
    pub(super) fn verify(&self, c: &Captured) -> Result<()> {
        self.current()?;
        let mut current = self.root.try_clone().map_err(|_| io())?;
        for (name, held) in &c.parents {
            let next = directory(&current, name.as_ref())?;
            if !hagency_platform::same_directory(held, &next).map_err(|_| io())? {
                return Err(io());
            }
            current = next;
        }
        let again = read(&current, c.relative_path.rsplit('/').next().ok_or_else(io)?)?;
        if identity(&again)? != c.identity
            || identity(&c.file)? != c.identity
            || !hagency_platform::same_file(&again, &c.file).map_err(|_| io())?
        {
            return Err(io());
        }
        Ok(())
    }
}
