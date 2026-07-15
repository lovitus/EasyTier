use std::{
    collections::HashSet,
    future::Future,
    os::fd::RawFd,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicU32, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use crate::{
    LeafPacketBridge, MeshServerResolver, PolicyRevision, PolicyRuntime, PolicyRuntimeBuildFuture,
    PolicyRuntimeFactory, compile_leaf_config,
};

const START_TIMEOUT: Duration = Duration::from_secs(3);
const STOP_TIMEOUT: Duration = Duration::from_secs(3);
static NEXT_RUNTIME_ID: AtomicU32 = AtomicU32::new(1);
static RESERVED_RUNTIME_IDS: LazyLock<Mutex<HashSet<leaf::RuntimeId>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone)]
pub struct InProcessLeafFactory {
    base_dir: PathBuf,
    resolver: Arc<dyn MeshServerResolver + Send + Sync>,
    dns_servers: Arc<[std::net::IpAddr]>,
    worker_threads: usize,
}

impl InProcessLeafFactory {
    pub fn new(
        base_dir: PathBuf,
        resolver: Arc<dyn MeshServerResolver + Send + Sync>,
        dns_servers: Vec<std::net::IpAddr>,
        worker_threads: usize,
    ) -> Result<Self, String> {
        if dns_servers.is_empty() {
            return Err("in-process Leaf requires at least one underlying DNS server".to_owned());
        }
        if worker_threads == 0 || worker_threads > 4 {
            return Err("in-process Leaf worker_threads must be in 1..=4".to_owned());
        }
        Ok(Self {
            base_dir,
            resolver,
            dns_servers: dns_servers.into(),
            worker_threads,
        })
    }

    pub async fn start(
        &self,
        revision: Arc<PolicyRevision>,
    ) -> Result<Arc<InProcessLeafRuntime>, String> {
        InProcessLeafRuntime::start(
            &self.base_dir,
            self.resolver.as_ref(),
            &self.dns_servers,
            self.worker_threads,
            revision,
        )
        .await
    }
}

pub struct InProcessLeafRuntime {
    revision_id: String,
    runtime_id: leaf::RuntimeId,
    bridge: Arc<LeafPacketBridge>,
    thread: Mutex<Option<JoinHandle<Result<(), String>>>>,
}

impl InProcessLeafRuntime {
    pub fn bridge(&self) -> Arc<LeafPacketBridge> {
        self.bridge.clone()
    }

    pub fn runtime_id(&self) -> leaf::RuntimeId {
        self.runtime_id
    }

    async fn start(
        base_dir: &std::path::Path,
        resolver: &(dyn MeshServerResolver + Send + Sync),
        dns_servers: &[std::net::IpAddr],
        worker_threads: usize,
        revision: Arc<PolicyRevision>,
    ) -> Result<Arc<Self>, String> {
        let (bridge, endpoint) = LeafPacketBridge::pair().map_err(|error| error.to_string())?;
        let packet_fd = LeafPacketFd::new(endpoint.into_raw_fd())?;
        let endpoint_fd = packet_fd.raw_fd();
        let config =
            match compile_leaf_config(&revision, endpoint_fd, base_dir, resolver, dns_servers) {
                Ok(config) => config,
                Err(error) => return Err(error.to_string()),
            };
        let mut config = match leaf::config::from_string(&config) {
            Ok(config) => config,
            Err(error) => return Err(format!("Leaf rejected generated config: {error}")),
        };
        disable_embedded_leaf_logger(&mut config);

        let runtime_reservation = allocate_runtime_id()?;
        let runtime_id = runtime_reservation.runtime_id();
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name(format!("easytier-leaf-{runtime_id}"))
            .spawn(move || {
                // These guards deliberately live for the complete blocking Leaf runtime. They
                // also clean up when Leaf panics or the OS rejects thread creation.
                let _runtime_reservation = runtime_reservation;
                let _packet_fd = packet_fd;
                let runtime_opt = if worker_threads == 1 {
                    leaf::RuntimeOption::SingleThread
                } else {
                    leaf::RuntimeOption::MultiThread(worker_threads, 2 * 1024 * 1024)
                };
                let result = leaf::start(
                    runtime_id,
                    leaf::StartOptions {
                        config: leaf::Config::Internal(config),
                        runtime_opt,
                    },
                )
                .map_err(|error| error.to_string());
                let _ = result_tx.send(result.clone());
                result
            })
            .map_err(|error| format!("failed to start in-process Leaf thread: {error}"))?;

        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if leaf::is_running(runtime_id) {
                return Ok(Arc::new(Self {
                    revision_id: revision.id.clone(),
                    runtime_id,
                    bridge: Arc::new(bridge),
                    thread: Mutex::new(Some(thread)),
                }));
            }
            match result_rx.try_recv() {
                Ok(Ok(())) => {
                    let _ = thread.join();
                    return Err("in-process Leaf exited before readiness".to_owned());
                }
                Ok(Err(error)) => {
                    let _ = thread.join();
                    return Err(format!("in-process Leaf startup failed: {error}"));
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = thread.join();
                    return Err("in-process Leaf startup thread exited unexpectedly".to_owned());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if Instant::now() >= deadline {
                let cleanup_deadline = Instant::now() + STOP_TIMEOUT;
                request_leaf_shutdown(runtime_id, cleanup_deadline).await;
                spawn_late_start_reaper(runtime_id, result_rx, thread);
                return Err("in-process Leaf readiness timed out".to_owned());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    pub fn is_running(&self) -> bool {
        leaf::is_running(self.runtime_id)
    }

    pub async fn stop(&self) {
        let deadline = Instant::now() + STOP_TIMEOUT;
        request_leaf_shutdown(self.runtime_id, deadline).await;
        while leaf::is_running(self.runtime_id) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let thread = lock_unpoisoned(&self.thread).take();
        if let Some(thread) = thread {
            if thread.is_finished() {
                let _ = tokio::task::spawn_blocking(move || thread.join()).await;
            } else {
                tracing::warn!(
                    runtime_id = self.runtime_id,
                    "in-process Leaf did not stop within the bounded shutdown window"
                );
                // Keep ownership of the thread handle even after the bounded caller-facing
                // shutdown returns. Tokio's blocking pool joins it once Leaf completes cleanup.
                drop(tokio::task::spawn_blocking(move || thread.join()));
            }
        }
    }
}

fn spawn_late_start_reaper(
    runtime_id: leaf::RuntimeId,
    result_rx: mpsc::Receiver<Result<(), String>>,
    thread: JoinHandle<Result<(), String>>,
) {
    // sing-box box.go::Start closes partially started services before returning an error. An
    // EasyTier in-process Leaf can register after our readiness deadline, so its cleanup owner
    // must outlive the failed start future even if the OS refuses another dedicated thread.
    let reaper = Arc::new(Mutex::new(Some(LateStartReaper {
        runtime_id,
        result_rx,
        thread,
    })));
    let thread_reaper = reaper.clone();
    let spawn_result = std::thread::Builder::new()
        .name(format!("easytier-leaf-reaper-{runtime_id}"))
        .spawn(move || {
            if let Some(reaper) = lock_unpoisoned(&thread_reaper).take() {
                reaper.run();
            }
        });
    if let Err(error) = spawn_result {
        tracing::warn!(
            runtime_id,
            %error,
            "failed to start dedicated in-process Leaf reaper; using blocking-pool fallback"
        );
        if let Some(reaper) = lock_unpoisoned(&reaper).take() {
            drop(tokio::task::spawn_blocking(move || reaper.run()));
        } else {
            tracing::error!(runtime_id, "in-process Leaf reaper ownership was lost");
        }
    }
}

struct LateStartReaper {
    runtime_id: leaf::RuntimeId,
    result_rx: mpsc::Receiver<Result<(), String>>,
    thread: JoinHandle<Result<(), String>>,
}

impl LateStartReaper {
    fn run(self) {
        loop {
            // Leaf registers the runtime near the end of synchronous startup. A shutdown issued
            // before registration is otherwise lost and would leave a late runtime detached.
            if leaf::is_running(self.runtime_id) {
                let _ = leaf::shutdown(self.runtime_id);
            }
            match self.result_rx.recv_timeout(Duration::from_millis(25)) {
                Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        if leaf::is_running(self.runtime_id) {
            let _ = leaf::shutdown(self.runtime_id);
        }
        let _ = self.thread.join();
    }
}

impl Drop for InProcessLeafRuntime {
    fn drop(&mut self) {
        if leaf::is_running(self.runtime_id) {
            let runtime_id = self.runtime_id;
            let _ = std::thread::Builder::new()
                .name(format!("easytier-leaf-drop-{runtime_id}"))
                .spawn(move || {
                    let _ = leaf::shutdown(runtime_id);
                });
        }
    }
}

async fn request_leaf_shutdown(runtime_id: leaf::RuntimeId, deadline: Instant) {
    // Leaf's public shutdown API uses blocking_send internally. Keep it off Tokio worker
    // threads so current-thread runtimes and Android lifecycle callbacks cannot panic. Share
    // the caller's stop deadline so a blocked dispatch cannot make bounded shutdown unbounded.
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        tracing::warn!(
            runtime_id,
            "in-process Leaf shutdown deadline elapsed before dispatch"
        );
        return;
    }
    match tokio::time::timeout(
        remaining,
        tokio::task::spawn_blocking(move || leaf::shutdown(runtime_id)),
    )
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => tracing::warn!(
            runtime_id,
            ?error,
            "failed to dispatch in-process Leaf shutdown"
        ),
        Err(_) => tracing::warn!(
            runtime_id,
            "in-process Leaf shutdown dispatch exceeded the bounded stop deadline"
        ),
    }
}

impl PolicyRuntime for InProcessLeafRuntime {
    fn revision_id(&self) -> &str {
        &self.revision_id
    }

    fn is_running(&self) -> bool {
        InProcessLeafRuntime::is_running(self)
    }

    fn shutdown(self: Arc<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move { self.stop().await })
    }
}

impl PolicyRuntimeFactory for InProcessLeafFactory {
    fn build(&self, revision: Arc<PolicyRevision>) -> PolicyRuntimeBuildFuture {
        let factory = self.clone();
        Box::pin(async move {
            factory
                .start(revision)
                .await
                .map(|runtime| runtime as Arc<dyn PolicyRuntime>)
        })
    }
}

fn disable_embedded_leaf_logger(config: &mut leaf::config::Config) {
    // EasyTier owns the process-wide tracing subscriber. Leaf's standalone logger attempts to
    // install another global subscriber, which panics when Leaf runs in-process on mobile.
    config.log.mut_or_insert_default().level = leaf::config::log::Level::NONE.into();
}

struct RuntimeIdReservation {
    runtime_id: leaf::RuntimeId,
}

impl RuntimeIdReservation {
    fn runtime_id(&self) -> leaf::RuntimeId {
        self.runtime_id
    }
}

impl Drop for RuntimeIdReservation {
    fn drop(&mut self) {
        lock_unpoisoned(&RESERVED_RUNTIME_IDS).remove(&self.runtime_id);
    }
}

fn allocate_runtime_id() -> Result<RuntimeIdReservation, String> {
    let mut reserved = lock_unpoisoned(&RESERVED_RUNTIME_IDS);
    for _ in 0..u16::MAX {
        let candidate = NEXT_RUNTIME_ID.fetch_add(1, Ordering::Relaxed) as u16;
        if candidate != 0 && !leaf::is_running(candidate) && reserved.insert(candidate) {
            return Ok(RuntimeIdReservation {
                runtime_id: candidate,
            });
        }
    }
    Err("no free in-process Leaf runtime ID".to_owned())
}

#[derive(Clone, Copy)]
struct FdIdentity {
    device: libc::dev_t,
    inode: libc::ino_t,
}

struct LeafPacketFd {
    fd: RawFd,
    identity: FdIdentity,
}

impl LeafPacketFd {
    fn new(fd: RawFd) -> Result<Self, String> {
        match fd_identity(fd) {
            Ok(identity) => Ok(Self { fd, identity }),
            Err(error) => {
                unsafe {
                    libc::close(fd);
                }
                Err(error)
            }
        }
    }

    fn raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl Drop for LeafPacketFd {
    fn drop(&mut self) {
        close_if_same_fd(self.fd, self.identity);
    }
}

fn fd_identity(fd: RawFd) -> Result<FdIdentity, String> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return Err(format!(
            "failed to inspect Leaf packet FD: {}",
            std::io::Error::last_os_error()
        ));
    }
    let stat = unsafe { stat.assume_init() };
    Ok(FdIdentity {
        device: stat.st_dev,
        inode: stat.st_ino,
    })
}

fn close_if_same_fd(fd: RawFd, expected: FdIdentity) {
    if fd_identity(fd)
        .is_ok_and(|actual| actual.device == expected.device && actual.inode == expected.inode)
    {
        unsafe {
            libc::close(fd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::IntoRawFd;

    fn unresolved_mesh(
        _proxy_name: &str,
        _instance_id: Option<uuid::Uuid>,
        _virtual_ip: Option<std::net::IpAddr>,
        _port: Option<u16>,
    ) -> Option<crate::ResolvedMeshServer> {
        None
    }

    #[tokio::test]
    async fn starts_and_stops_with_unique_runtime_and_external_packet_fd() {
        let revision = Arc::new(
            PolicyRevision::parse(
                "version: 1\nrules: [\"FINAL,DIRECT\"]\n",
                std::path::Path::new("."),
            )
            .unwrap(),
        );
        let factory = InProcessLeafFactory::new(
            PathBuf::from("."),
            Arc::new(unresolved_mesh),
            vec!["1.1.1.1".parse().unwrap()],
            1,
        )
        .unwrap();

        let runtime = factory.start(revision).await.unwrap();
        assert_ne!(runtime.runtime_id(), 0);
        assert!(runtime.is_running());
        runtime.stop().await;
        assert!(!runtime.is_running());
    }

    #[test]
    fn rejects_unbounded_or_dns_less_runtime_options() {
        assert!(InProcessLeafFactory::new(
            PathBuf::from("."),
            Arc::new(unresolved_mesh),
            Vec::new(),
            1,
        )
        .is_err());
        assert!(
            InProcessLeafFactory::new(
                PathBuf::from("."),
                Arc::new(unresolved_mesh),
                vec!["1.1.1.1".parse().unwrap()],
                5,
            )
            .is_err()
        );
    }

    #[test]
    fn embedded_leaf_does_not_replace_the_process_logger() {
        let mut config = leaf::config::Config::new();
        disable_embedded_leaf_logger(&mut config);
        assert_eq!(config.log.level, leaf::config::log::Level::NONE.into());
    }

    #[test]
    fn runtime_id_reservation_is_released_by_drop() {
        let reservation = allocate_runtime_id().unwrap();
        let runtime_id = reservation.runtime_id();
        assert!(lock_unpoisoned(&RESERVED_RUNTIME_IDS).contains(&runtime_id));
        drop(reservation);
        assert!(!lock_unpoisoned(&RESERVED_RUNTIME_IDS).contains(&runtime_id));
    }

    #[test]
    fn packet_fd_guard_closes_an_unclaimed_descriptor() {
        let (socket, _peer) = std::os::unix::net::UnixDatagram::pair().unwrap();
        let fd = socket.into_raw_fd();
        let guard = LeafPacketFd::new(fd).unwrap();
        assert_eq!(guard.raw_fd(), fd);
        drop(guard);
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFD) }, -1);
    }

    #[test]
    fn late_start_reaper_retains_and_joins_start_thread() {
        let reservation = allocate_runtime_id().unwrap();
        let runtime_id = reservation.runtime_id();
        drop(reservation);
        let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_completed = completed.clone();
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || {
            thread_completed.store(true, Ordering::Release);
            let result = Ok(());
            let _ = result_tx.send(result.clone());
            result
        });
        LateStartReaper {
            runtime_id,
            result_rx,
            thread,
        }
        .run();
        assert!(completed.load(Ordering::Acquire));
    }
}
