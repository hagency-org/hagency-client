//! Private share staging on one observable blocking worker. Native share APIs do
//! not report completion: presented files remain for 30 minutes (minute sweeps),
//! or until account retirement/shutdown. Failed presentation is discarded sooner.
use crate::cache_dir;
use matrix_sdk::ruma::OwnedUserId;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Condvar, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
#[path = "private_share_files.rs"]
mod files;

pub(crate) const SHARE_TTL: Duration = Duration::from_secs(30 * 60);
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);
const OBSERVATION_INTERVAL: Duration = Duration::from_millis(100);
const MAX_JOBS: usize = 8;
const MAX_STAGED_BYTES: usize = 256 * 1024 * 1024;
const MAX_STAGED_FILES: usize = 32;

#[derive(Clone, Debug)]
pub(crate) struct ShareScope {
    epoch: u64,
    owner: OwnedUserId,
}
impl ShareScope {
    pub fn capture() -> Option<Self> {
        let scope = Self {
            epoch: crate::account_session::epoch(),
            owner: crate::sliding_sync::current_user_id()?,
        };
        scope.is_current().then_some(scope)
    }
    pub fn is_current(&self) -> bool {
        !WORKER.stopped.load(Ordering::Acquire)
            && self.epoch == crate::account_session::epoch()
            && crate::sliding_sync::current_user_id().as_ref() == Some(&self.owner)
            && !crate::logout::logout_state_machine::is_logout_in_progress()
    }
}
#[derive(Debug)]
pub(crate) struct ShareTicket {
    id: uuid::Uuid,
    scope: ShareScope,
    pub path: PathBuf,
    expires: Instant,
    live: AtomicBool,
}
impl ShareTicket {
    pub fn begin_presentation(&self) -> bool {
        let mut state = WORKER.state.lock().unwrap();
        if !self.is_current() || !state.claim_presentation() {
            return false;
        }
        true
    }
    pub fn presentation_failed(&self) {
        let mut state = WORKER.state.lock().unwrap();
        state.presenting = state.presenting.saturating_sub(1);
    }
    pub fn is_current(&self) -> bool {
        self.live.load(Ordering::Acquire)
            && Instant::now() < self.expires
            && self.scope.is_current()
    }
    pub fn observe(self: &Arc<Self>, mut pending: robius_share::PendingShare) {
        let mut state = WORKER.state.lock().unwrap();
        state.presenting = state.presenting.saturating_sub(1);
        if state.finished {
            if !matches!(pending.cancel(), Ok(true)) {
                state.cleanup_failures += 1;
                state.observations.push_back((self.clone(), pending));
                makepad_widgets::error!(
                    "Late share observation retained after worker failure; retirement remains unconfirmed"
                );
            }
            return;
        }
        state.observations.push_back((self.clone(), pending));
        WORKER.wake.notify_one();
    }
    pub fn discard(&self) {
        self.live.store(false, Ordering::Release);
        let mut state = WORKER.state.lock().unwrap();
        state.discard.insert(self.id);
        WORKER.wake.notify_one();
    }
}
type Callback = Box<dyn FnOnce(Result<Arc<ShareTicket>, String>) + Send>;
struct Job {
    scope: ShareScope,
    filename: String,
    bytes: Arc<[u8]>,
    ready: Callback,
}
struct Stored {
    ticket: Arc<ShareTicket>,
    file: files::StagedFile,
}
#[derive(Default)]
struct State {
    queue: VecDeque<Job>,
    discard: BTreeSet<uuid::Uuid>,
    retire_before: u64,
    retire_dirty: bool,
    used_bytes: usize,
    stored: usize,
    busy: bool,
    finished: bool,
    cleanup_failures: usize,
    orphan_failures: usize,
    poisoned: bool,
    observations: VecDeque<(Arc<ShareTicket>, robius_share::PendingShare)>,
    native_observations: usize,
    presenting: usize,
}
impl State {
    fn retained_native(&self) -> usize {
        self.native_observations + self.observations.len() + self.presenting
    }
    fn claim_presentation(&mut self) -> bool {
        if self.retained_native() >= MAX_STAGED_FILES {
            return false;
        }
        self.presenting += 1;
        true
    }
    fn admits(&self, bytes: usize) -> bool {
        self.retained_native() + self.queue.len() + usize::from(self.busy) < MAX_STAGED_FILES
            && !self.poisoned
            && self.queue.len() + usize::from(self.busy) < MAX_JOBS
            && self.stored + self.queue.len() + usize::from(self.busy) < MAX_STAGED_FILES
            && bytes <= MAX_STAGED_BYTES.saturating_sub(self.used_bytes)
    }
}
struct Worker {
    state: Mutex<State>,
    wake: Condvar,
    stopped: AtomicBool,
    join: Mutex<Option<JoinHandle<()>>>,
}
static WORKER: LazyLock<Arc<Worker>> = LazyLock::new(|| {
    let worker = Arc::new(Worker {
        state: Mutex::new(State::default()),
        wake: Condvar::new(),
        stopped: AtomicBool::new(false),
        join: Mutex::new(None),
    });
    let running = worker.clone();
    let cache = cache_dir().to_path_buf();
    match std::thread::Builder::new()
        .name("hagency-private-shares".into())
        .spawn(move || {
            let observer = running.clone();
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_worker(running, cache)))
                .is_err()
            {
                makepad_widgets::error!(
                    "Private share worker panicked; deletion remains unconfirmed"
                );
                observer.stopped.store(true, Ordering::Release);
                let mut state = observer
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                state.finished = true;
                state.cleanup_failures += 1;
                observer.wake.notify_all();
            }
        }) {
        Ok(join) => *worker.join.lock().unwrap() = Some(join),
        Err(error) => {
            makepad_widgets::error!("Could not start private share worker: {error}");
            worker.stopped.store(true, Ordering::Release);
            worker.state.lock().unwrap().finished = true;
        }
    }
    worker
});

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Diagnostic snapshot: counts only, never paths or content.
pub(crate) struct ShareStorageStats {
    pub queued: usize,
    pub running: bool,
    pub staged: usize,
    pub reserved_bytes: usize,
    pub cleanup_failures: usize,
    pub native_observations: usize,
    pub poisoned: bool,
    pub stopped: bool,
    pub finished: bool,
}
#[allow(dead_code)]
pub(crate) fn share_storage_stats() -> ShareStorageStats {
    let state = WORKER.state.lock().unwrap();
    ShareStorageStats {
        queued: state.queue.len(),
        running: state.busy,
        staged: state.stored,
        reserved_bytes: state.used_bytes,
        cleanup_failures: state.cleanup_failures,
        native_observations: state.retained_native(),
        poisoned: state.poisoned,
        stopped: WORKER.stopped.load(Ordering::Acquire),
        finished: state.finished,
    }
}

/// Initialize crash cleanup without doing synchronous disk work on the UI/Tokio.
pub fn schedule_temp_dir_cleanup() {
    let _ = &*WORKER;
}

pub(crate) fn stage_share(
    scope: ShareScope,
    filename: String,
    bytes: Arc<[u8]>,
    ready: Callback,
) -> Result<(), String> {
    if !scope.is_current() {
        return Err("Share account is no longer active".into());
    }
    let mut state = WORKER.state.lock().unwrap();
    if state.poisoned {
        return Err("Private share storage requires cleanup recovery".into());
    }
    if WORKER.stopped.load(Ordering::Acquire) || !state.admits(bytes.len()) {
        return Err("Private share staging is busy or exceeds its storage limit".into());
    }
    state.used_bytes += bytes.len();
    state.queue.push_back(Job {
        scope,
        filename,
        bytes,
        ready,
    });
    WORKER.wake.notify_one();
    Ok(())
}

/// Called after the existing account close/drain barrier, before opening a new
/// account. Queued/running jobs still check their original epoch after disk IO.
pub fn retire_account_shares() {
    let mut state = WORKER.state.lock().unwrap();
    state.retire_before = state.retire_before.max(crate::account_session::epoch());
    state.retire_dirty = true;
    WORKER.wake.notify_one();
}

/// UI shutdown only: observe/join the original worker, with a bounded wait.
/// A timeout/failure is reported; it is not represented as completed deletion.
pub fn shutdown_temp_storage() {
    let state = WORKER.state.lock().unwrap();
    // The predicate and notification share the worker's wait mutex. Otherwise
    // stop can be signalled between its predicate check and Condvar sleep.
    WORKER.stopped.store(true, Ordering::Release);
    WORKER.wake.notify_one();
    let (state, _) = WORKER
        .wake
        .wait_timeout_while(state, Duration::from_secs(5), |state| !state.finished)
        .unwrap();
    if !state.finished || state.cleanup_failures != 0 {
        makepad_widgets::error!(
            "Private share cleanup incomplete: worker finished={}, remaining cleanup failures={}",
            state.finished,
            state.cleanup_failures
        );
    }
    let finished = state.finished;
    drop(state);
    if finished {
        if let Some(join) = WORKER.join.lock().unwrap().take() {
            if join.join().is_err() {
                makepad_widgets::error!("Private share worker panicked during shutdown");
            }
        }
    }
}

#[derive(Debug)]
enum StageFailure {
    Rejected(String),
    Storage(files::StageError),
}
impl StageFailure {
    fn cleanup_unknown(&self) -> bool {
        matches!(self, Self::Storage(error) if error.cleanup_unknown)
    }
    fn message(&self) -> String {
        match self {
            Self::Rejected(message) => message.clone(),
            Self::Storage(_) => "Could not create private share file".into(),
        }
    }
}

fn stage_checked(
    root: &files::RunDirectory,
    scope: &ShareScope,
    filename: &str,
    bytes: &[u8],
    valid: impl Fn(&ShareScope) -> bool,
) -> Result<Stored, StageFailure> {
    if !valid(scope) {
        return Err(StageFailure::Rejected(
            "Share account is no longer active".into(),
        ));
    }
    let file = root.stage(filename, bytes, SHARE_TTL).map_err(|error| {
        makepad_widgets::error!("Private share preparation failed: {error}");
        StageFailure::Storage(error)
    })?;
    let ticket = Arc::new(ShareTicket {
        id: uuid::Uuid::new_v4(),
        scope: scope.clone(),
        path: file.path.clone(),
        expires: file.expires,
        live: AtomicBool::new(valid(scope)),
    });
    Ok(Stored { ticket, file })
}

fn cleanup_entries(
    stored: &mut BTreeMap<uuid::Uuid, Stored>,
    discard: &BTreeSet<uuid::Uuid>,
    retire_before: u64,
    stop: bool,
    now: Instant,
) -> (usize, usize) {
    let mut removed_bytes = 0;
    stored.retain(|id, entry| {
        let remove = stop
            || discard.contains(id)
            || entry.ticket.scope.epoch < retire_before
            || now >= entry.file.expires
            || !entry.ticket.live.load(Ordering::Acquire);
        if !remove {
            return true;
        }
        entry.ticket.live.store(false, Ordering::Release);
        match entry.file.remove() {
            Ok(()) => {
                removed_bytes += entry.file.bytes;
                false
            }
            Err(error) => {
                makepad_widgets::error!("Private share cleanup pending: {error}");
                true
            }
        }
    });
    (
        removed_bytes,
        stored
            .values()
            .filter(|entry| !entry.ticket.live.load(Ordering::Acquire))
            .count(),
    )
}

fn record_stage_failure(state: &mut State, bytes: usize, cleanup_unknown: bool) {
    if cleanup_unknown {
        state.orphan_failures += 1;
        state.cleanup_failures += 1;
        state.poisoned = true;
        // Reservation still covers actual leftover plaintext. Fail closed until
        // the next process's original-handle crash recovery observes deletion.
    } else {
        state.used_bytes = state.used_bytes.saturating_sub(bytes);
    }
}

fn run_worker(worker: Arc<Worker>, cache: PathBuf) {
    let mut root = match files::RunDirectory::open(&cache, SHARE_TTL) {
        Ok(root) => Some(root),
        Err(error) => {
            makepad_widgets::error!("Private share storage unavailable: {error}");
            None
        }
    };
    if root.is_some() {
        if let Err(error) = files::sweep_legacy(&cache, SHARE_TTL) {
            makepad_widgets::error!("Legacy share cleanup incomplete: {error}");
        }
    }
    let mut stored = BTreeMap::<uuid::Uuid, Stored>::new();
    let mut last_sweep = Instant::now();
    let mut observations = Vec::<(Arc<ShareTicket>, robius_share::PendingShare)>::new();
    loop {
        if last_sweep.elapsed() >= SWEEP_INTERVAL {
            if let Err(error) = files::sweep_legacy(&cache, SHARE_TTL) {
                makepad_widgets::error!("Legacy share cleanup incomplete: {error}");
            }
            last_sweep = Instant::now();
        }
        let (job, discard, retire_before, stop, incoming) = {
            let mut state = worker.state.lock().unwrap();
            if state.queue.is_empty()
                && state.discard.is_empty()
                && !state.retire_dirty
                && state.observations.is_empty()
                && !worker.stopped.load(Ordering::Acquire)
            {
                state = worker
                    .wake
                    .wait_timeout(
                        state,
                        if observations.is_empty() {
                            SWEEP_INTERVAL
                        } else {
                            OBSERVATION_INTERVAL
                        },
                    )
                    .unwrap()
                    .0;
            }
            let stop = worker.stopped.load(Ordering::Acquire);
            state.retire_dirty = false;
            let job = state.queue.pop_front();
            state.busy = job.is_some();
            // Move incoming observations into the live count under the same
            // mutex, so admission never sees a gap while the worker polls them.
            let incoming_count = state.observations.len();
            state.native_observations += incoming_count;
            (
                job,
                std::mem::take(&mut state.discard),
                state.retire_before,
                stop,
                std::mem::take(&mut state.observations),
            )
        };
        observations.extend(incoming);
        let mut observation_errors = 0;
        observations.retain_mut(|(ticket, pending)| {
            let retire = stop
                || ticket.scope.epoch < retire_before
                || !ticket.live.load(Ordering::Acquire)
                || Instant::now() >= ticket.expires;
            let result = if retire {
                pending.cancel()
            } else {
                match pending.poll() {
                    Ok(done) => Ok(done),
                    Err(error) => {
                        makepad_widgets::error!(
                            "Native share observation is unknown, no fallback/retry: {error}"
                        );
                        pending.cancel()
                    }
                }
            };
            match result {
                Ok(done) => !done,
                Err(error) => {
                    makepad_widgets::error!("Native share observation retirement pending: {error}");
                    observation_errors += 1;
                    true
                }
            }
        });
        let (removed_bytes, cleanup_failures) =
            cleanup_entries(&mut stored, &discard, retire_before, stop, Instant::now());
        {
            let mut state = worker.state.lock().unwrap();
            state.used_bytes = state.used_bytes.saturating_sub(removed_bytes);
            state.stored = stored.len();
            state.cleanup_failures = cleanup_failures + observation_errors + state.orphan_failures;
            state.native_observations = observations.len();
        }
        if let Some(job) = job {
            let bytes = job.bytes.len();
            let poisoned = worker.state.lock().unwrap().poisoned;
            let result = if stop || poisoned {
                Err(StageFailure::Rejected(if poisoned {
                    "Private share storage requires cleanup recovery".into()
                } else {
                    "Share application is shutting down".into()
                }))
            } else {
                root.as_ref()
                    .ok_or_else(|| {
                        StageFailure::Rejected("Private share storage is unavailable".to_owned())
                    })
                    .and_then(|root| {
                        stage_checked(
                            root,
                            &job.scope,
                            &job.filename,
                            &job.bytes,
                            ShareScope::is_current,
                        )
                    })
            };
            match result {
                Ok(entry) => {
                    let ready =
                        entry.ticket.live.load(Ordering::Acquire) && entry.file.verify().is_ok();
                    let ticket = entry.ticket.clone();
                    if !ready {
                        entry.ticket.live.store(false, Ordering::Release);
                        worker.state.lock().unwrap().discard.insert(ticket.id);
                    }
                    stored.insert(ticket.id, entry);
                    {
                        let mut state = worker.state.lock().unwrap();
                        state.stored = stored.len();
                    }
                    (job.ready)(if ready {
                        Ok(ticket)
                    } else {
                        Err("Share account or file changed during preparation".into())
                    });
                }
                Err(error) => {
                    {
                        let mut state = worker.state.lock().unwrap();
                        record_stage_failure(&mut state, bytes, error.cleanup_unknown());
                    }
                    (job.ready)(Err(error.message()));
                }
            }
            worker.state.lock().unwrap().busy = false;
        }
        if stop {
            let state = worker.state.lock().unwrap();
            if !state.queue.is_empty() || !state.observations.is_empty() || !observations.is_empty()
            {
                drop(state);
                std::thread::sleep(OBSERVATION_INTERVAL);
                continue;
            }
            drop(state);
            if let Some(run) = root.take() {
                if let Err(error) = run.remove() {
                    makepad_widgets::error!(
                        "Private share shutdown directory cleanup failed: {error}"
                    );
                    worker.state.lock().unwrap().cleanup_failures += 1;
                }
            }
            let mut state = worker.state.lock().unwrap();
            state.finished = true;
            worker.wake.notify_all();
            return;
        }
    }
}

#[cfg(test)]
#[path = "private_share_tests.rs"]
mod tests;
