//! App-side bounded buffering. The SDK send queue still owns its separate buffers.
use crate::transfer_budget::{ByteBudget, Reservation};
use std::{
    fs::File,
    io::{Read, self},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, Condvar,
        atomic::{AtomicBool, Ordering},
    },
    collections::VecDeque,
};
use tokio::sync::oneshot;

pub(crate) const MAX_UPLOAD_BYTES: u64 = 64 * 1024 * 1024;
const TOTAL_BYTES: u64 = 2 * MAX_UPLOAD_BYTES;
const MAX_QUEUED: usize = 16;
const BUFFER: usize = 64 * 1024;

/// Retain the selected file identity through the bounded preparation queue.
#[derive(Clone, Debug)]
pub(crate) struct Selection {
    identity: Arc<same_file::Handle>,
    size: u64,
    modified: Option<std::time::SystemTime>,
}
impl Selection {
    pub fn capture(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(io::Error::other("Upload requires a regular file"));
        }
        Ok(Self {
            identity: Arc::new(same_file::Handle::from_file(file)?),
            size: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}
pub(crate) struct UploadBytes {
    pub bytes: Vec<u8>,
    // Keep this object until the original SDK use_send_queue() has returned.
    pub reservation: Reservation,
}
struct Job {
    path: PathBuf,
    selection: Selection,
    limit: u64,
    live: Arc<AtomicBool>,
    current: Arc<dyn Fn() -> bool + Send + Sync>,
    done: oneshot::Sender<Result<UploadBytes, String>>,
}
struct Service {
    queue: Mutex<VecDeque<Job>>,
    wake: Condvar,
    bytes: Arc<ByteBudget>,
    stopped: AtomicBool,
    joins: Mutex<Vec<std::thread::JoinHandle<()>>>,
}
static SERVICE: std::sync::OnceLock<Arc<Service>> = std::sync::OnceLock::new();
fn service() -> &'static Arc<Service> {
    SERVICE.get_or_init(|| {
        let service = Arc::new(Service {
            queue: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
            bytes: Arc::default(),
            stopped: AtomicBool::new(false),
            joins: Mutex::default(),
        });
        for index in 0..2 {
            let running = service.clone();
            let join = std::thread::Builder::new()
                .name(format!("attachment-upload-{index}"))
                .spawn(move || {
                    loop {
                        let job = {
                            let mut queue = running.queue.lock().unwrap();
                            while queue.is_empty() && !running.stopped.load(Ordering::Acquire) {
                                queue = running.wake.wait(queue).unwrap();
                            }
                            if running.stopped.load(Ordering::Acquire) {
                                break;
                            }
                            queue.pop_front().unwrap()
                        };
                        let valid = || {
                            !running.stopped.load(Ordering::Acquire)
                                && job.live.load(Ordering::Acquire)
                                && (job.current)()
                        };
                        let result = read_source(
                            &job.path,
                            &job.selection,
                            job.limit,
                            &running.bytes,
                            valid,
                        );
                        // A cancelled observer cannot free the original read's reservation.
                        let _ = job.done.send(result);
                        running.wake.notify_all();
                    }
                })
                .expect("bounded upload worker");
            service.joins.lock().unwrap().push(join);
        }
        service
    })
}
struct Cancel(Arc<AtomicBool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub(crate) async fn read(
    path: PathBuf,
    selection: Selection,
    server_limit: Option<u64>,
    current: impl Fn() -> bool + Send + Sync + 'static,
) -> Result<UploadBytes, String> {
    let live = Arc::new(AtomicBool::new(true));
    let _cancel = Cancel(live.clone());
    let (done, receiver) = oneshot::channel();
    {
        let mut queue = service().queue.lock().unwrap();
        if service().stopped.load(Ordering::Acquire) || queue.len() >= MAX_QUEUED {
            return Err("Attachment upload preparation is busy".into());
        }
        queue.push_back(Job {
            path,
            selection,
            limit: server_limit
                .unwrap_or(MAX_UPLOAD_BYTES)
                .min(MAX_UPLOAD_BYTES),
            live,
            current: Arc::new(current),
            done,
        });
        service().wake.notify_one();
    }
    receiver
        .await
        .map_err(|_| "Attachment upload worker stopped".to_owned())?
}

pub(crate) fn shutdown(deadline: std::time::Instant) {
    let Some(service) = SERVICE.get() else {
        return;
    };
    let mut queue = service.queue.lock().unwrap();
    service.stopped.store(true, Ordering::Release);
    queue.clear();
    service.wake.notify_all();
    while !service
        .joins
        .lock()
        .unwrap()
        .iter()
        .all(|join| join.is_finished())
    {
        let now = std::time::Instant::now();
        if now >= deadline {
            makepad_widgets::error!(
                "Upload source shutdown unconfirmed: {} bytes retained",
                service.bytes.used()
            );
            return;
        }
        queue = service
            .wake
            .wait_timeout(
                queue,
                (deadline - now).min(std::time::Duration::from_millis(50)),
            )
            .unwrap()
            .0;
    }
    drop(queue);
    for join in service.joins.lock().unwrap().drain(..) {
        if join.join().is_err() {
            makepad_widgets::error!("Original upload source worker failed");
        }
    }
}

fn read_source(
    path: &Path,
    selection: &Selection,
    limit: u64,
    budget: &Arc<ByteBudget>,
    valid: impl Fn() -> bool,
) -> Result<UploadBytes, String> {
    let operation = || -> io::Result<UploadBytes> {
        if !valid() {
            return Err(io::Error::other("Upload account is no longer active"));
        }
        let mut file = File::open(path)?;
        let before = file.metadata()?;
        let id = same_file::Handle::from_file(file.try_clone()?)?;
        if !before.is_file()
            || &id != selection.identity.as_ref()
            || before.len() != selection.size
            || before.modified().ok() != selection.modified
        {
            return Err(io::Error::other(
                "Selected upload file changed; choose it again",
            ));
        }
        if before.len() > limit {
            return Err(io::Error::other(
                "Attachment exceeds the upload limit (application maximum: 64 MiB)",
            ));
        }
        let reservation = budget
            .reserve(before.len(), TOTAL_BYTES)
            .map_err(io::Error::other)?;
        let mut bytes = Vec::with_capacity(before.len() as usize);
        let mut block = [0u8; BUFFER];
        loop {
            if !valid() {
                return Err(io::Error::other("Upload cancelled or account retired"));
            }
            let count = file.read(&mut block)?;
            if count == 0 {
                break;
            }
            if count as u64 > limit.saturating_sub(bytes.len() as u64)
                || count as u64 > before.len().saturating_sub(bytes.len() as u64)
            {
                return Err(io::Error::other("Upload file grew while reading"));
            }
            bytes.extend_from_slice(&block[..count]);
        }
        let after = file.metadata()?;
        let current = same_file::Handle::from_path(path)?;
        if bytes.len() as u64 != before.len()
            || before.len() != after.len()
            || before.modified().ok() != after.modified().ok()
            || id != current
            || !valid()
        {
            return Err(io::Error::other(
                "Upload file changed or account retired while reading",
            ));
        }
        Ok(UploadBytes { bytes, reservation })
    };
    operation().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn actual_source_rejects_growth_truncation_replacement_and_stale_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source");
        std::fs::write(&path, b"selected").unwrap();
        let selected = Selection::capture(&path).unwrap();
        let budget = Arc::new(ByteBudget::default());
        assert_eq!(
            read_source(&path, &selected, 8, &budget, || true)
                .unwrap()
                .bytes,
            b"selected"
        );
        for changed in [b"larger than selected".as_slice(), b"tiny".as_slice()] {
            std::fs::write(&path, changed).unwrap();
            assert!(read_source(&path, &selected, MAX_UPLOAD_BYTES, &budget, || true).is_err());
        }
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"selected").unwrap();
        assert!(read_source(&path, &selected, MAX_UPLOAD_BYTES, &budget, || true).is_err());
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn actual_reads_obey_independent_limit_and_cancellation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source");
        std::fs::write(&path, b"eight888").unwrap();
        let selected = Selection::capture(&path).unwrap();
        let budget = Arc::new(ByteBudget::default());
        assert!(read_source(&path, &selected, 7, &budget, || true).is_err());
        assert!(read_source(&path, &selected, MAX_UPLOAD_BYTES, &budget, || false).is_err());
        let bytes = read_source(&path, &selected, MAX_UPLOAD_BYTES, &budget, || true).unwrap();
        assert_eq!(budget.used(), 8);
        drop(bytes);
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn real_source_mutations_after_first_read_are_rejected() {
        use std::sync::atomic::AtomicUsize;
        for mutation in 0..3 {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("source");
            std::fs::write(&path, vec![7u8; 3 * BUFFER]).unwrap();
            let selection = Selection::capture(&path).unwrap();
            let budget = Arc::new(ByteBudget::default());
            let calls = AtomicUsize::new(0);
            let result = read_source(&path, &selection, MAX_UPLOAD_BYTES, &budget, || {
                if calls.fetch_add(1, Ordering::AcqRel) == 2 {
                    match mutation {
                        0 => {
                            std::fs::OpenOptions::new()
                                .append(true)
                                .open(&path)
                                .unwrap()
                                .write_all(b"growth")
                                .unwrap();
                        }
                        1 => {
                            std::fs::OpenOptions::new()
                                .write(true)
                                .open(&path)
                                .unwrap()
                                .set_len(10)
                                .unwrap();
                        }
                        _ => {
                            std::fs::rename(&path, directory.path().join("original")).unwrap();
                            std::fs::write(&path, vec![7u8; 3 * BUFFER]).unwrap();
                        }
                    }
                }
                true
            });
            assert!(
                result.is_err(),
                "mutation during original IO must not enqueue changed bytes"
            );
            assert_eq!(budget.used(), 0);
        }
    }
    #[test]
    fn absent_server_limit_still_rejects_actual_source_above_application_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large");
        File::create(&path)
            .unwrap()
            .set_len(MAX_UPLOAD_BYTES + 1)
            .unwrap();
        let selection = Selection::capture(&path).unwrap();
        let budget = Arc::new(ByteBudget::default());
        assert!(read_source(&path, &selection, MAX_UPLOAD_BYTES, &budget, || true).is_err());
        assert_eq!(budget.used(), 0);
    }
}
