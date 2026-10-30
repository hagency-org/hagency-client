//! Isolated real-network/private-file probe. See tools/test_attachment_memory_soak.py.
//! Fixture encryption and SDK initialization finish before the allocator window.
#[cfg(all(feature = "heap-profile", feature = "agent_chat"))]
mod probe {
    use std::{
        fs::File,
        io::{self, Read, Write},
        net::{TcpListener, TcpStream},
        path::{Path, PathBuf},
        sync::{
            Arc, Condvar, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };
    use matrix_sdk::{
        Client,
        ruma::{api::MatrixVersion, events::room::MediaSource},
    };
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    const BUFFER: usize = 64 * 1024;

    struct Pattern {
        remaining: u64,
        position: u64,
    }
    impl Read for Pattern {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            let length = output.len().min(self.remaining as usize);
            for (offset, byte) in output[..length].iter_mut().enumerate() {
                *byte = ((self.position + offset as u64) % 251) as u8;
            }
            self.position += length as u64;
            self.remaining -= length as u64;
            Ok(length)
        }
    }
    struct HashSink {
        hash: Sha256,
        bytes: u64,
    }
    impl HashSink {
        fn new() -> Self {
            Self {
                hash: Sha256::new(),
                bytes: 0,
            }
        }
        fn finish(self) -> String {
            format!("{:x}", self.hash.finalize())
        }
    }
    impl Write for HashSink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.hash.update(bytes);
            self.bytes += bytes.len() as u64;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    fn copy_bounded(reader: &mut impl Read, writer: &mut impl Write) -> io::Result<u64> {
        let mut bytes = [0u8; BUFFER];
        let mut count = 0;
        loop {
            let length = reader.read(&mut bytes)?;
            if length == 0 {
                return Ok(count);
            }
            writer.write_all(&bytes[..length])?;
            count += length as u64;
        }
    }
    fn fixture(path: &Path, length: u64, encrypted: bool) -> io::Result<(Option<Value>, String)> {
        // Neither the source nor ciphertext is ever represented as a whole Vec.
        let mut expected = HashSink::new();
        copy_bounded(
            &mut Pattern {
                remaining: length,
                position: 0,
            },
            &mut expected,
        )?;
        let mut file = File::create(path)?;
        let descriptor = if encrypted {
            let mut pattern = Pattern {
                remaining: length,
                position: 0,
            };
            let mut encryptor = matrix_sdk_base::crypto::AttachmentEncryptor::new(&mut pattern);
            assert_eq!(copy_bounded(&mut encryptor, &mut file)?, length);
            Some(serde_json::to_value(encryptor.finish()).map_err(io::Error::other)?)
        } else {
            assert_eq!(
                copy_bounded(
                    &mut Pattern {
                        remaining: length,
                        position: 0
                    },
                    &mut file
                )?,
                length
            );
            None
        };
        file.sync_all()?;
        Ok((descriptor, expected.finish()))
    }

    struct Barrier {
        goal: usize,
        arrived: usize,
    }
    struct Observation {
        active: AtomicUsize,
        peak: AtomicUsize,
        authenticated: AtomicUsize,
        barrier: Mutex<Barrier>,
        ready: Condvar,
    }
    impl Observation {
        fn arm(&self, goal: usize) {
            assert_eq!(self.active.load(Ordering::Acquire), 0);
            *self.barrier.lock().unwrap() = Barrier { goal, arrived: 0 };
            self.peak.store(0, Ordering::Release);
            self.authenticated.store(0, Ordering::Release);
        }
        fn start(&self) -> io::Result<()> {
            let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
            self.peak.fetch_max(active, Ordering::AcqRel);
            let mut barrier = self.barrier.lock().unwrap();
            barrier.arrived += 1;
            self.ready.notify_all();
            let (barrier, timeout) = self
                .ready
                .wait_timeout_while(barrier, Duration::from_secs(15), |state| {
                    state.arrived < state.goal
                })
                .unwrap();
            if timeout.timed_out() && barrier.arrived < barrier.goal {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "production workers did not reach the concurrent-body barrier",
                ));
            }
            Ok(())
        }
    }
    struct Active(Arc<Observation>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.active.fetch_sub(1, Ordering::AcqRel);
        }
    }
    struct Server {
        url: String,
        observed: Arc<Observation>,
        stopped: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }
    impl Server {
        fn new(fixture: PathBuf) -> io::Result<Self> {
            let listener = TcpListener::bind("127.0.0.1:0")?;
            listener.set_nonblocking(true)?;
            let url = format!("http://{}", listener.local_addr()?);
            let observed = Arc::new(Observation {
                active: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                authenticated: AtomicUsize::new(0),
                barrier: Mutex::new(Barrier {
                    goal: 1,
                    arrived: 0,
                }),
                ready: Condvar::new(),
            });
            let stopped = Arc::new(AtomicBool::new(false));
            let observation = observed.clone();
            let stop = stopped.clone();
            let thread = std::thread::spawn(move || {
                let mut workers = Vec::new();
                while !stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((socket, _)) => {
                            // Fixed fixture cardinality: SDK setup + one warmup + four bodies.
                            assert!(workers.len() < 24, "unexpected SDK request storm");
                            let path = fixture.clone();
                            let observed = observation.clone();
                            workers
                                .push(std::thread::spawn(move || serve(socket, &path, observed)));
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2))
                        }
                        Err(error) => panic!("fixture accept failed: {error}"),
                    }
                }
                for worker in workers {
                    worker.join().unwrap().unwrap();
                }
            });
            Ok(Self {
                url,
                observed,
                stopped,
                thread: Some(thread),
            })
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.stopped.store(true, Ordering::Release);
            self.thread.take().unwrap().join().unwrap();
        }
    }
    fn serve(mut socket: TcpStream, fixture: &Path, observed: Arc<Observation>) -> io::Result<()> {
        socket.set_nonblocking(false)?;
        socket.set_read_timeout(Some(Duration::from_secs(15)))?;
        socket.set_write_timeout(Some(Duration::from_secs(60)))?;
        let mut request = Vec::with_capacity(1024);
        let mut byte = [0u8; 1];
        while !request.ends_with(b"\r\n\r\n") && request.len() < 16384 {
            if socket.read(&mut byte)? == 0 {
                return Ok(());
            }
            request.push(byte[0]);
        }
        let request = String::from_utf8(request).map_err(io::Error::other)?;
        if !request
            .lines()
            .next()
            .is_some_and(|line| line.contains("/download/example.org/"))
        {
            return socket.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
            );
        }
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer attachment-probe"),
            "the real authenticated endpoint must carry SDK credentials"
        );
        observed.authenticated.fetch_add(1, Ordering::AcqRel);
        let warmup = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|target| target.split('?').next())
            .is_some_and(|path| path.ends_with("/warmup"));
        let mut file = File::open(fixture)?;
        let length = if warmup {
            BUFFER as u64
        } else {
            file.metadata()?.len()
        };
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
        )?;
        // Count the body before waiting, so a serialized client fails the test.
        let started = observed.start();
        let _active = Active(observed);
        started?;
        assert_eq!(
            copy_bounded(&mut (&mut file).take(length), &mut socket)?,
            length
        );
        Ok(())
    }

    async fn settle() -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let stats = hagency_app::attachment_transport::download_stats();
            if stats.active == 0
                && stats.queued == 0
                && stats.stored_bytes == 0
                && stats.cleanup_failures == 0
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(
                    "original worker IO/private-file cleanup did not settle within 10 seconds"
                        .into(),
                );
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    fn source(descriptor: &Option<Value>, id: usize) -> MediaSource {
        let uri = format!("mxc://example.org/fixture-{id}");
        match descriptor {
            Some(value) => {
                let mut value = value.clone();
                value["url"] = uri.into();
                MediaSource::Encrypted(Box::new(serde_json::from_value(value).unwrap()))
            }
            None => MediaSource::Plain(uri.into()),
        }
    }
    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let arguments = std::env::args().skip(1).collect::<Vec<_>>();
        let argument = |name: &str| -> &str {
            let index = arguments
                .iter()
                .position(|value| value == name)
                .expect("missing probe argument");
            arguments.get(index + 1).expect("missing probe value")
        };
        let length: u64 = argument("--bytes").parse()?;
        assert!(matches!(length, 33_554_432 | 268_435_456));
        let concurrency: usize = argument("--concurrency").parse()?;
        assert!(matches!(concurrency, 1 | 4));
        let kind = argument("--kind");
        assert!(matches!(kind, "plain" | "encrypted"));
        let directory = PathBuf::from(argument("--output"));
        std::fs::create_dir_all(&directory)?;
        let cache = directory.join("private-cache");
        let fixture_path = directory.join("source.fixture");
        let (descriptor, expected_hash) = fixture(&fixture_path, length, kind == "encrypted")?;
        let server = Server::new(fixture_path)?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let client = Client::builder().http_client(matrix_sdk::reqwest::Client::builder().no_proxy().build()?)
                .homeserver_url(&server.url).server_versions([MatrixVersion::V1_11]).build().await?;
            client.matrix_auth().restore_session(serde_json::from_value(json!({"access_token":"attachment-probe", "user_id":"@probe:example.org", "device_id":"MEMORY-PROBE"}))?, Default::default()).await?;
            tokio::time::timeout(Duration::from_secs(15), client.encryption().wait_for_e2ee_initialization_tasks()).await?;
            let warmup = hagency_app::attachment_transport::fetch_for_probe(client.clone(), MediaSource::Plain("mxc://example.org/warmup".into()), &cache).await?;
            assert_eq!(warmup.len(), BUFFER as u64);
            drop(warmup);
            settle().await?;
            while server.observed.active.load(Ordering::Acquire) != 0 { tokio::time::sleep(Duration::from_millis(1)).await; }
            server.observed.arm(concurrency);

            // A fresh profiler in each process measures every allocation during
            // the real transfer, including transient buffers, not just retained bytes.
            let profiler = dhat::Profiler::builder().file_name(directory.join("heap.json")).build();
            let started = Instant::now();
            let jobs = (0..concurrency).map(|index| {
                hagency_app::attachment_transport::fetch_for_probe(client.clone(), source(&descriptor, index), &cache)
            });
            let results = futures_util::future::join_all(jobs).await;
            let mut hashes = Vec::with_capacity(concurrency);
            let mut actual_bytes = 0;
            for result in &results {
                let file = result.as_ref().map_err(|error| io::Error::other(error.to_string()))?;
                assert_eq!(file.len(), length);
                let mut sink = HashSink::new();
                assert_eq!(file.read_to(&mut sink)?, length);
                actual_bytes += sink.bytes;
                let hash = sink.finish();
                assert_eq!(hash, expected_hash, "only verified exact plaintext may become usable");
                hashes.push(hash);
            }
            let retained = hagency_app::attachment_transport::download_stats();
            let retained_evidence = (retained.active, retained.queued, retained.stored_bytes, retained.cleanup_failures);
            let completed = results.len();
            drop(results);
            settle().await?;
            let stats = hagency_app::attachment_transport::download_stats();
            let heap = dhat::HeapStats::get();
            let elapsed = started.elapsed().as_secs_f64();
            drop(profiler);
            let result = json!({"kind":kind, "bytes_per_file":length, "concurrency":concurrency,
                "completed":completed, "verified":true, "actual_plaintext_bytes":actual_bytes,
                "hashes":hashes, "expected_sha256":expected_hash,
                "heap_peak_bytes":heap.max_bytes, "heap_retained_bytes":heap.curr_bytes,
                "heap_total_allocated_bytes":heap.total_bytes, "elapsed_seconds":elapsed,
                "server_peak_active":server.observed.peak.load(Ordering::Acquire),
                "authenticated_downloads":server.observed.authenticated.load(Ordering::Acquire),
                "held_quota":{"active":retained_evidence.0,"queued":retained_evidence.1,"stored_bytes":retained_evidence.2,"cleanup_failures":retained_evidence.3},
                "final_quota":{"active":stats.active,"queued":stats.queued,"stored_bytes":stats.stored_bytes,"cleanup_failures":stats.cleanup_failures},
                "cleanup_confirmed":true, "explicit_buffer_bytes":BUFFER,
                "scope":"real production transport with probe-only account predicate; fixture and SDK initialization outside allocator window"});
            std::fs::write(directory.join("result.json"), serde_json::to_vec_pretty(&result)?)?;
            Ok::<(), Box<dyn std::error::Error>>(())
        })?;
        Ok(())
    }
}

#[cfg(all(feature = "heap-profile", feature = "agent_chat"))]
#[global_allocator]
static HEAP_ALLOCATOR: dhat::Alloc = dhat::Alloc;

#[cfg(all(feature = "heap-profile", feature = "agent_chat"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    probe::run()
}

#[cfg(not(all(feature = "heap-profile", feature = "agent_chat")))]
fn main() {
    panic!("attachment_memory_soak requires --features heap-profile,agent_chat");
}
