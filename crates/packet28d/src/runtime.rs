use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use packet28_daemon_core::task_store_lease::TaskStoreLease;
use tokio::sync::{watch, Notify, OwnedSemaphorePermit, Semaphore};

const DEFAULT_MAX_CONNECTIONS: usize = 128;
const DEFAULT_MAX_BLOCKING_OPERATIONS: usize = 8;
pub(crate) const CONTROL_BLOCKING_OPERATIONS: usize = 2;
pub(crate) const CANCELLATION_BLOCKING_OPERATIONS: usize = 2;
const DEFAULT_MAX_PERSISTENT_ROOTS: usize = 8;
const DEFAULT_SUBSCRIBER_QUEUE_CAPACITY: usize = 256;
const DEFAULT_WATCH_QUEUE_CAPACITY: usize = 1_024;
const DEFAULT_BACKGROUND_QUEUE_CAPACITY: usize = 64;
const DEFAULT_FRAME_READ_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_FRAME_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DaemonRuntimeConfig {
    pub(crate) max_connections: usize,
    pub(crate) max_blocking_operations: usize,
    pub(crate) max_persistent_roots: usize,
    pub(crate) subscriber_queue_capacity: usize,
    pub(crate) watch_queue_capacity: usize,
    pub(crate) background_queue_capacity: usize,
    pub(crate) frame_header_timeout: Duration,
    pub(crate) frame_body_timeout: Duration,
    pub(crate) frame_write_timeout: Duration,
    pub(crate) shutdown_grace: Duration,
}

impl Default for DaemonRuntimeConfig {
    fn default() -> Self {
        Self {
            max_connections: DEFAULT_MAX_CONNECTIONS,
            max_blocking_operations: DEFAULT_MAX_BLOCKING_OPERATIONS,
            max_persistent_roots: DEFAULT_MAX_PERSISTENT_ROOTS,
            subscriber_queue_capacity: DEFAULT_SUBSCRIBER_QUEUE_CAPACITY,
            watch_queue_capacity: DEFAULT_WATCH_QUEUE_CAPACITY,
            background_queue_capacity: DEFAULT_BACKGROUND_QUEUE_CAPACITY,
            frame_header_timeout: DEFAULT_FRAME_READ_TIMEOUT,
            frame_body_timeout: DEFAULT_FRAME_READ_TIMEOUT,
            frame_write_timeout: DEFAULT_FRAME_WRITE_TIMEOUT,
            shutdown_grace: DEFAULT_SHUTDOWN_GRACE,
        }
    }
}

impl DaemonRuntimeConfig {
    pub(crate) fn from_env() -> Result<Self> {
        let defaults = Self::default();
        Ok(Self {
            max_connections: env_nonzero_usize(
                "PACKET28D_MAX_CONNECTIONS",
                defaults.max_connections,
            )?,
            max_blocking_operations: env_nonzero_usize(
                "PACKET28D_MAX_BLOCKING_OPERATIONS",
                defaults.max_blocking_operations,
            )?,
            max_persistent_roots: env_nonzero_usize(
                "PACKET28D_MAX_PERSISTENT_ROOTS",
                defaults.max_persistent_roots,
            )?,
            subscriber_queue_capacity: env_nonzero_usize(
                "PACKET28D_SUBSCRIBER_QUEUE_CAPACITY",
                defaults.subscriber_queue_capacity,
            )?,
            watch_queue_capacity: env_nonzero_usize(
                "PACKET28D_WATCH_QUEUE_CAPACITY",
                defaults.watch_queue_capacity,
            )?,
            background_queue_capacity: env_nonzero_usize(
                "PACKET28D_BACKGROUND_QUEUE_CAPACITY",
                defaults.background_queue_capacity,
            )?,
            frame_header_timeout: env_nonzero_duration_ms(
                "PACKET28D_FRAME_HEADER_TIMEOUT_MS",
                defaults.frame_header_timeout,
            )?,
            frame_body_timeout: env_nonzero_duration_ms(
                "PACKET28D_FRAME_BODY_TIMEOUT_MS",
                defaults.frame_body_timeout,
            )?,
            frame_write_timeout: env_nonzero_duration_ms(
                "PACKET28D_FRAME_WRITE_TIMEOUT_MS",
                defaults.frame_write_timeout,
            )?,
            shutdown_grace: env_nonzero_duration_ms(
                "PACKET28D_SHUTDOWN_GRACE_MS",
                defaults.shutdown_grace,
            )?,
        })
    }
}

fn env_nonzero_usize(name: &str, default: usize) -> Result<usize> {
    let Some(raw) = std::env::var_os(name) else {
        return Ok(default);
    };
    let raw = raw
        .into_string()
        .map_err(|_| anyhow!("{name} must contain valid UTF-8"))?;
    let value = raw
        .parse::<usize>()
        .with_context(|| format!("{name} must be a positive integer"))?;
    if value == 0 {
        anyhow::bail!("{name} must be greater than zero");
    }
    Ok(value)
}

fn env_nonzero_duration_ms(name: &str, default: Duration) -> Result<Duration> {
    let millis = env_nonzero_usize(
        name,
        usize::try_from(default.as_millis()).unwrap_or(usize::MAX),
    )?;
    let millis = u64::try_from(millis).with_context(|| format!("{name} is too large"))?;
    Ok(Duration::from_millis(millis))
}

#[derive(Debug, Clone)]
pub(crate) struct ShutdownSignal {
    sender: watch::Sender<bool>,
}

impl Default for ShutdownSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl ShutdownSignal {
    pub(crate) fn new() -> Self {
        let (sender, _receiver) = watch::channel(false);
        Self { sender }
    }

    pub(crate) fn request(&self) {
        self.sender.send_replace(true);
    }

    pub(crate) fn is_requested(&self) -> bool {
        *self.sender.borrow()
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<bool> {
        self.sender.subscribe()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StateChangeSignal {
    sender: watch::Sender<u64>,
}

impl Default for StateChangeSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl StateChangeSignal {
    pub(crate) fn new() -> Self {
        let (sender, _receiver) = watch::channel(0);
        Self { sender }
    }

    pub(crate) fn notify(&self) {
        let next = self.sender.borrow().wrapping_add(1);
        self.sender.send_replace(next);
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.sender.subscribe()
    }
}

/// Bounds all CPU-heavy or synchronous work admitted from the async runtime.
///
/// The owned permit moves into the blocking closure. Cancelling its async
/// caller therefore cannot accidentally admit replacement work while the
/// original closure is still running.
#[derive(Debug, Clone)]
pub(crate) struct BlockingPool {
    inner: Arc<BlockingPoolInner>,
}

#[derive(Debug)]
struct BlockingPoolInner {
    permits: Arc<Semaphore>,
    // Control permits are reserved for short request decoding and response
    // encoding so Stop can progress while data or cancellation work is full.
    control_permits: Arc<Semaphore>,
    // Task cancellation can wait for owned work and children; keep that
    // bounded without occupying the short control codec lane.
    cancellation_permits: Arc<Semaphore>,
    max_operations: usize,
    admission: std::sync::Mutex<()>,
    shutting_down: AtomicBool,
    active_operations: AtomicUsize,
    idle: Notify,
    daemon_instance_lease: Option<TaskStoreLease>,
    task_store_lease: Option<TaskStoreLease>,
}

/// Cooperative cancellation view passed to admitted blocking work.
#[derive(Debug, Clone)]
pub(crate) struct BlockingCancellation {
    inner: Arc<BlockingPoolInner>,
}

impl BlockingCancellation {
    pub(crate) fn is_cancelled(&self) -> bool {
        self.inner.shutting_down.load(Ordering::Acquire)
    }
}

struct BlockingActivity {
    inner: Arc<BlockingPoolInner>,
}

impl Drop for BlockingActivity {
    fn drop(&mut self) {
        let previous = self.inner.active_operations.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "blocking activity counter underflowed");
        if previous == 1 {
            self.inner.idle.notify_waiters();
        }
    }
}

/// A blocking-operation slot reserved before an async worker is spawned.
///
/// Reservations count as active work, so dropping an unconsumed admission
/// releases both its permit and its idle-barrier activity.
pub(crate) struct BlockingAdmission {
    permit: OwnedSemaphorePermit,
    activity: BlockingActivity,
    cancellation: BlockingCancellation,
    daemon_instance_lease: Option<TaskStoreLease>,
    task_store_lease: Option<TaskStoreLease>,
}

impl BlockingAdmission {
    pub(crate) async fn run_cancellable<F, T>(self, operation: F) -> Result<T>
    where
        F: FnOnce(BlockingCancellation) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let Self {
            permit,
            activity,
            cancellation,
            daemon_instance_lease,
            task_store_lease,
        } = self;
        tokio::task::spawn_blocking(move || {
            let _activity = activity;
            let _permit = permit;
            let _daemon_instance_lease = daemon_instance_lease;
            let _task_store_lease = task_store_lease;
            if cancellation.is_cancelled() {
                anyhow::bail!("daemon blocking operation cancelled before start");
            }
            operation(cancellation)
        })
        .await
        .map_err(|error| anyhow!("daemon blocking operation failed to join: {error}"))?
    }
}

impl BlockingPool {
    #[cfg(test)]
    pub(crate) fn new(max_operations: usize) -> Self {
        Self::with_optional_lifecycle_leases(max_operations, None, None)
    }

    pub(crate) fn with_lifecycle_leases(
        max_operations: usize,
        daemon_instance_lease: TaskStoreLease,
        task_store_lease: TaskStoreLease,
    ) -> Self {
        Self::with_optional_lifecycle_leases(
            max_operations,
            Some(daemon_instance_lease),
            Some(task_store_lease),
        )
    }

    fn with_optional_lifecycle_leases(
        max_operations: usize,
        daemon_instance_lease: Option<TaskStoreLease>,
        task_store_lease: Option<TaskStoreLease>,
    ) -> Self {
        assert!(
            max_operations > 0,
            "blocking operation capacity must be nonzero"
        );
        Self {
            inner: Arc::new(BlockingPoolInner {
                permits: Arc::new(Semaphore::new(max_operations)),
                control_permits: Arc::new(Semaphore::new(CONTROL_BLOCKING_OPERATIONS)),
                cancellation_permits: Arc::new(Semaphore::new(CANCELLATION_BLOCKING_OPERATIONS)),
                max_operations,
                admission: std::sync::Mutex::new(()),
                shutting_down: AtomicBool::new(false),
                active_operations: AtomicUsize::new(0),
                idle: Notify::new(),
                daemon_instance_lease,
                task_store_lease,
            }),
        }
    }

    pub(crate) async fn run<F, T>(&self, operation: F) -> Result<T>
    where
        F: FnOnce() -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.run_cancellable(move |_| operation()).await
    }

    pub(crate) async fn run_cancellable<F, T>(&self, operation: F) -> Result<T>
    where
        F: FnOnce(BlockingCancellation) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.admit().await?.run_cancellable(operation).await
    }

    pub(crate) async fn run_control<F, T>(&self, operation: F) -> Result<T>
    where
        F: FnOnce() -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.admit_control()
            .await?
            .run_cancellable(move |_| operation())
            .await
    }

    pub(crate) async fn run_cancellation<F, T>(&self, operation: F) -> Result<T>
    where
        F: FnOnce() -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.admit_cancellation()
            .await?
            .run_cancellable(move |_| operation())
            .await
    }

    /// Waits for capacity and reserves it before a caller creates an async
    /// worker that will own the blocking operation.
    pub(crate) async fn admit(&self) -> Result<BlockingAdmission> {
        self.admit_from(self.inner.permits.clone()).await
    }

    async fn admit_control(&self) -> Result<BlockingAdmission> {
        self.admit_from(self.inner.control_permits.clone()).await
    }

    async fn admit_cancellation(&self) -> Result<BlockingAdmission> {
        self.admit_from(self.inner.cancellation_permits.clone())
            .await
    }

    async fn admit_from(&self, permits: Arc<Semaphore>) -> Result<BlockingAdmission> {
        if self.is_shutting_down() {
            anyhow::bail!("daemon blocking executor is shutting down");
        }
        let permit = permits
            .acquire_owned()
            .await
            .map_err(|_| anyhow!("daemon blocking executor is closed"))?;
        let admission = self
            .inner
            .admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.is_shutting_down() {
            anyhow::bail!("daemon blocking executor is shutting down");
        }
        self.inner.active_operations.fetch_add(1, Ordering::AcqRel);
        drop(admission);
        Ok(BlockingAdmission {
            permit,
            activity: BlockingActivity {
                inner: self.inner.clone(),
            },
            cancellation: BlockingCancellation {
                inner: self.inner.clone(),
            },
            daemon_instance_lease: self.inner.daemon_instance_lease.clone(),
            task_store_lease: self.inner.task_store_lease.clone(),
        })
    }

    pub(crate) fn max_operations(&self) -> usize {
        self.inner.max_operations
    }

    pub(crate) fn request_shutdown(&self) {
        let admission = self
            .inner
            .admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.inner.shutting_down.store(true, Ordering::Release);
        self.inner.permits.close();
        self.inner.control_permits.close();
        self.inner.cancellation_permits.close();
        drop(admission);
        self.inner.idle.notify_waiters();
    }

    pub(crate) fn is_shutting_down(&self) -> bool {
        self.inner.shutting_down.load(Ordering::Acquire)
    }

    pub(crate) async fn wait_for_idle(&self) {
        loop {
            let idle = self.inner.idle.notified();
            tokio::pin!(idle);
            idle.as_mut().enable();
            if self.active_operations() == 0 {
                return;
            }
            idle.await;
        }
    }

    pub(crate) fn active_operations(&self) -> usize {
        self.inner.active_operations.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn available_permits(&self) -> usize {
        self.inner.permits.available_permits()
    }

    #[cfg(test)]
    pub(crate) fn available_control_permits(&self) -> usize {
        self.inner.control_permits.available_permits()
    }

    #[cfg(test)]
    pub(crate) fn available_cancellation_permits(&self) -> usize {
        self.inner.cancellation_permits.available_permits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn runtime_defaults_are_nonzero_and_bounded() {
        let config = DaemonRuntimeConfig::default();
        assert!(config.max_connections > 0);
        assert!(config.max_blocking_operations > 0);
        assert!(config.max_persistent_roots > 0);
        assert!(config.subscriber_queue_capacity > 0);
        assert!(config.watch_queue_capacity > 0);
        assert!(config.background_queue_capacity > 0);
        assert!(!config.frame_header_timeout.is_zero());
        assert!(!config.frame_body_timeout.is_zero());
        assert!(!config.frame_write_timeout.is_zero());
        assert!(!config.shutdown_grace.is_zero());
    }

    #[tokio::test]
    async fn blocking_permit_remains_owned_until_closure_finishes() {
        let pool = BlockingPool::new(1);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let task_pool = pool.clone();
        let task = tokio::spawn(async move {
            task_pool
                .run(move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .await
        });

        started_rx.await.unwrap();
        assert_eq!(pool.available_permits(), 0);
        task.abort();
        tokio::task::yield_now().await;
        assert_eq!(pool.available_permits(), 0);
        release_tx.send(()).unwrap();
        for _ in 0..100 {
            if pool.available_permits() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(pool.available_permits(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bounded_blocking_work_does_not_starve_runtime_timers() {
        let pool = BlockingPool::new(1);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let worker = tokio::spawn(async move {
            pool.run(move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
            .await
        });
        started_rx.await.unwrap();

        tokio::time::timeout(Duration::from_millis(250), async {
            tokio::time::sleep(Duration::from_millis(10)).await;
        })
        .await
        .expect("blocking daemon work starved the async timer");

        release_tx.send(()).unwrap();
        worker.await.unwrap().unwrap();
    }

    fn runtime_timer_lateness_samples(isolated: bool) -> Vec<u128> {
        const ITERATIONS: usize = 32;
        const TIMER_DELAY: Duration = Duration::from_millis(1);
        const BLOCKING_DURATION: Duration = Duration::from_millis(10);

        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(async {
                let pool = BlockingPool::new(1);
                let mut samples = Vec::with_capacity(ITERATIONS);
                for _ in 0..ITERATIONS {
                    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
                    let timer = tokio::spawn(async move {
                        let started = Instant::now();
                        started_tx.send(()).unwrap();
                        tokio::time::sleep(TIMER_DELAY).await;
                        started.elapsed().saturating_sub(TIMER_DELAY).as_micros()
                    });
                    started_rx.await.unwrap();
                    if isolated {
                        pool.run(|| {
                            std::thread::sleep(BLOCKING_DURATION);
                            Ok(())
                        })
                        .await
                        .unwrap();
                    } else {
                        std::thread::sleep(BLOCKING_DURATION);
                    }
                    samples.push(timer.await.unwrap());
                }
                samples
            })
    }

    fn percentile(samples: &[u128], percentile: usize) -> u128 {
        let index = samples.len().saturating_sub(1).saturating_mul(percentile) / 100;
        samples[index]
    }

    #[test]
    #[ignore = "release-only ASY-04 benchmark; run explicitly with --ignored --nocapture"]
    fn benchmark_runtime_timer_starvation_boundary() {
        let mut direct = runtime_timer_lateness_samples(false);
        let mut isolated = runtime_timer_lateness_samples(true);
        direct.sort_unstable();
        isolated.sort_unstable();

        println!(
            "{{\"iterations\":{},\"timer_delay_us\":1000,\"blocking_duration_us\":10000,\
             \"direct_sync_p50_lateness_us\":{},\"direct_sync_p95_lateness_us\":{},\
             \"direct_sync_max_lateness_us\":{},\"blocking_pool_p50_lateness_us\":{},\
             \"blocking_pool_p95_lateness_us\":{},\"blocking_pool_max_lateness_us\":{}}}",
            direct.len(),
            percentile(&direct, 50),
            percentile(&direct, 95),
            direct.last().copied().unwrap_or_default(),
            percentile(&isolated, 50),
            percentile(&isolated, 95),
            isolated.last().copied().unwrap_or_default(),
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn saturated_cancellation_work_does_not_starve_the_control_codec_lane() {
        let pool = BlockingPool::new(1);
        let (started_tx, started_rx) =
            std::sync::mpsc::sync_channel(CANCELLATION_BLOCKING_OPERATIONS);
        let mut releases = Vec::new();
        let mut workers = Vec::new();
        for _ in 0..CANCELLATION_BLOCKING_OPERATIONS {
            let worker_pool = pool.clone();
            let started_tx = started_tx.clone();
            let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
            releases.push(release_tx);
            workers.push(tokio::spawn(async move {
                worker_pool
                    .run_cancellation(move || {
                        started_tx.send(()).unwrap();
                        release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                        Ok(())
                    })
                    .await
            }));
        }
        drop(started_tx);
        for _ in 0..CANCELLATION_BLOCKING_OPERATIONS {
            started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        assert_eq!(pool.available_cancellation_permits(), 0);
        assert_eq!(
            pool.available_control_permits(),
            CONTROL_BLOCKING_OPERATIONS
        );

        tokio::time::timeout(Duration::from_millis(250), pool.run_control(|| Ok(())))
            .await
            .expect("control codec lane was starved by cancellation work")
            .unwrap();

        for release in releases {
            release.send(()).unwrap();
        }
        for worker in workers {
            worker.await.unwrap().unwrap();
        }
    }

    #[tokio::test]
    async fn admission_reserves_capacity_before_async_worker_spawn() {
        let pool = BlockingPool::new(1);
        let admission = pool.admit().await.unwrap();
        assert_eq!(pool.active_operations(), 1);
        assert_eq!(pool.available_permits(), 0);

        let waiting_pool = pool.clone();
        let waiting = tokio::spawn(async move { waiting_pool.run(|| Ok(())).await });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());

        drop(admission);
        waiting.await.unwrap().unwrap();
        pool.wait_for_idle().await;
        assert_eq!(pool.active_operations(), 0);
    }

    #[tokio::test]
    async fn shutdown_cancels_reserved_work_before_its_closure_starts() {
        let pool = BlockingPool::new(1);
        let admission = pool.admit().await.unwrap();
        let ran = Arc::new(AtomicBool::new(false));
        pool.request_shutdown();

        let closure_ran = ran.clone();
        let result = admission
            .run_cancellable(move |_| {
                closure_ran.store(true, Ordering::Release);
                Ok(())
            })
            .await;

        assert!(result.is_err());
        assert!(!ran.load(Ordering::Acquire));
        pool.wait_for_idle().await;
        assert_eq!(pool.active_operations(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_cancels_waiters_and_tracks_started_work_until_exit() {
        let pool = BlockingPool::new(1);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let worker_pool = pool.clone();
        let worker = tokio::spawn(async move {
            worker_pool
                .run_cancellable(move |cancellation| {
                    started_tx.send(cancellation.clone()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .await
        });
        let cancellation = started_rx.await.unwrap();
        assert_eq!(pool.active_operations(), 1);

        let waiting_pool = pool.clone();
        let waiter = tokio::spawn(async move { waiting_pool.run(|| Ok(())).await });
        tokio::task::yield_now().await;
        pool.request_shutdown();

        assert!(pool.is_shutting_down());
        assert!(cancellation.is_cancelled());
        assert!(waiter.await.unwrap().is_err());
        assert_eq!(pool.active_operations(), 1);
        assert!(
            tokio::time::timeout(Duration::from_millis(25), pool.wait_for_idle())
                .await
                .is_err(),
            "idle barrier completed while admitted blocking work was still running"
        );

        release_tx.send(()).unwrap();
        worker.await.unwrap().unwrap();
        tokio::time::timeout(Duration::from_secs(1), pool.wait_for_idle())
            .await
            .expect("blocking activity did not become idle");
        assert_eq!(pool.active_operations(), 0);
    }
}
