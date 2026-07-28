use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tokio::sync::{watch, Semaphore};

const DEFAULT_MAX_CONNECTIONS: usize = 128;
const DEFAULT_MAX_BLOCKING_OPERATIONS: usize = 8;
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
    permits: Arc<Semaphore>,
}

impl BlockingPool {
    pub(crate) fn new(max_operations: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(max_operations)),
        }
    }

    pub(crate) async fn run<F, T>(&self, operation: F) -> Result<T>
    where
        F: FnOnce() -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let permit = self
            .permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| anyhow!("daemon blocking executor is closed"))?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            operation()
        })
        .await
        .map_err(|error| anyhow!("daemon blocking operation failed to join: {error}"))?
    }

    #[cfg(test)]
    pub(crate) fn available_permits(&self) -> usize {
        self.permits.available_permits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_defaults_are_nonzero_and_bounded() {
        let config = DaemonRuntimeConfig::default();
        assert!(config.max_connections > 0);
        assert!(config.max_blocking_operations > 0);
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
}
