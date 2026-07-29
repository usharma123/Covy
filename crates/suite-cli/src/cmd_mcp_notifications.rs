use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use packet28_daemon_core::storage::{load_task_events_from_offset, TaskEventLogRead};
use serde_json::{json, Map, Value};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use super::transport::McpMessageFraming;
use super::McpSessionState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NotificationDelivery {
    Delivered,
    Backpressured,
}

/// Owns notification cancellation and task completion.
pub(super) struct NotificationTask {
    shutdown: watch::Sender<bool>,
    task: Option<JoinHandle<Result<()>>>,
}

impl NotificationTask {
    pub(super) fn request_shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    pub(super) async fn shutdown(mut self) -> Result<()> {
        self.request_shutdown();
        self.join().await
    }

    pub(super) async fn join(&mut self) -> Result<()> {
        let task = self
            .task
            .take()
            .ok_or_else(|| anyhow!("MCP notification task was already joined"))?;
        task.await
            .map_err(|error| anyhow!("MCP notification task failed: {error}"))?
    }
}

impl Drop for NotificationTask {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub(super) fn start_notification_task<Deliver, DeliveryFuture>(
    root: PathBuf,
    session: Arc<Mutex<McpSessionState>>,
    poll_interval: Duration,
    deliver: Deliver,
) -> NotificationTask
where
    Deliver: FnMut(Value, McpMessageFraming) -> DeliveryFuture + Send + 'static,
    DeliveryFuture: Future<Output = Result<NotificationDelivery>> + Send + 'static,
{
    start_notification_task_with_reader(
        root,
        session,
        poll_interval,
        |root, task_id, offset| async move {
            let read_task_id = task_id.clone();
            match tokio::task::spawn_blocking(move || {
                load_task_events_from_offset(&root, &read_task_id, offset)
            })
            .await
            {
                Ok(Ok(read)) => Ok(Some(read)),
                Ok(Err(error)) => Err(anyhow!(
                    "MCP notification event-log read failed for task {task_id:?} at offset {offset}: {error}"
                )),
                Err(error) => Err(anyhow!("MCP notification reader task failed: {error}")),
            }
        },
        deliver,
    )
}

fn start_notification_task_with_reader<Read, ReadFuture, Deliver, DeliveryFuture>(
    root: PathBuf,
    session: Arc<Mutex<McpSessionState>>,
    poll_interval: Duration,
    read: Read,
    deliver: Deliver,
) -> NotificationTask
where
    Read: FnMut(PathBuf, String, u64) -> ReadFuture + Send + 'static,
    ReadFuture: Future<Output = Result<Option<TaskEventLogRead>>> + Send + 'static,
    Deliver: FnMut(Value, McpMessageFraming) -> DeliveryFuture + Send + 'static,
    DeliveryFuture: Future<Output = Result<NotificationDelivery>> + Send + 'static,
{
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let task = tokio::spawn(run_notification_loop(
        root,
        session,
        poll_interval,
        shutdown_receiver,
        read,
        deliver,
    ));
    NotificationTask {
        shutdown,
        task: Some(task),
    }
}

async fn run_notification_loop<Read, ReadFuture, Deliver, DeliveryFuture>(
    root: PathBuf,
    session: Arc<Mutex<McpSessionState>>,
    poll_interval: Duration,
    mut shutdown: watch::Receiver<bool>,
    mut read: Read,
    mut deliver: Deliver,
) -> Result<()>
where
    Read: FnMut(PathBuf, String, u64) -> ReadFuture,
    ReadFuture: Future<Output = Result<Option<TaskEventLogRead>>>,
    Deliver: FnMut(Value, McpMessageFraming) -> DeliveryFuture,
    DeliveryFuture: Future<Output = Result<NotificationDelivery>>,
{
    let mut interval = tokio::time::interval(poll_interval);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
            _ = interval.tick() => {
                tokio::select! {
                    biased;
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            return Ok(());
                        }
                    }
                    result = run_notification_pass(&root, &session, &mut read, &mut deliver) => {
                        result?;
                    }
                }
            }
        }
    }
}

async fn run_notification_pass<Read, ReadFuture, Deliver, DeliveryFuture>(
    root: &Path,
    session: &Arc<Mutex<McpSessionState>>,
    read: &mut Read,
    deliver: &mut Deliver,
) -> Result<()>
where
    Read: FnMut(PathBuf, String, u64) -> ReadFuture,
    ReadFuture: Future<Output = Result<Option<TaskEventLogRead>>>,
    Deliver: FnMut(Value, McpMessageFraming) -> DeliveryFuture,
    DeliveryFuture: Future<Output = Result<NotificationDelivery>>,
{
    let (initialized, tracked_tasks, tracked_task_offsets, framing) = match session.lock() {
        Ok(guard) => (
            guard.initialized,
            guard.tracked_tasks.clone(),
            guard.tracked_task_offsets.clone(),
            guard.framing,
        ),
        Err(_) => return Err(anyhow!("MCP notification session lock is poisoned")),
    };
    let Some(framing) = framing.filter(|_| initialized) else {
        return Ok(());
    };

    for (task_id, last_seen_seq) in tracked_tasks {
        let previous_offset = tracked_task_offsets.get(&task_id).copied().unwrap_or(0);
        let read = match read(root.to_path_buf(), task_id.clone(), previous_offset).await {
            Ok(Some(read)) => read,
            Ok(None) => continue,
            Err(error) => {
                return Err(error);
            }
        };
        let mut newest_delivered_seq = last_seen_seq;
        let mut backpressured = false;
        for frame in read
            .events
            .into_iter()
            .filter(|frame| frame.seq > last_seen_seq)
        {
            if frame.event.kind != "context_updated" {
                newest_delivered_seq = newest_delivered_seq.max(frame.seq);
                continue;
            }
            let mut params = match frame.event.data {
                Value::Object(map) => map,
                other => {
                    let mut map = Map::new();
                    map.insert("data".to_string(), other);
                    map
                }
            };
            params.insert("task_id".to_string(), Value::String(task_id.clone()));
            params.insert(
                "context_version".to_string(),
                params
                    .get("context_version")
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            params.insert("event_seq".to_string(), Value::Number(frame.seq.into()));
            let notification = json!({
                "jsonrpc":"2.0",
                "method":"notifications/packet28.context_updated",
                "params": Value::Object(params),
            });
            match deliver(notification, framing).await {
                Ok(NotificationDelivery::Delivered) => {}
                Ok(NotificationDelivery::Backpressured) => {
                    backpressured = true;
                    break;
                }
                Err(error) => {
                    return Err(anyhow!(
                        "MCP notification delivery failed for task {task_id:?}: {error}"
                    ));
                }
            }
            newest_delivered_seq = newest_delivered_seq.max(frame.seq);
        }
        if newest_delivered_seq > last_seen_seq
            || (!backpressured && read.next_offset != previous_offset)
        {
            let Ok(mut guard) = session.lock() else {
                return Err(anyhow!("MCP notification session lock is poisoned"));
            };
            if let Some(current) = guard.tracked_tasks.get_mut(&task_id) {
                *current = newest_delivered_seq;
            }
            if !backpressured {
                guard.tracked_task_offsets.insert(task_id, read.next_offset);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use packet28_daemon_core::storage::save_task_registry;
    use packet28_daemon_protocol::message::{DaemonEvent, DaemonEventFrame};
    use packet28_daemon_protocol::paths::{task_event_log_path, TaskStorageId};
    use packet28_daemon_protocol::task::{TaskRecord, TaskRegistry};
    use tempfile::TempDir;

    use super::*;
    use crate::cmd_mcp::proxy_upstream::proxy_output_channel;

    const TASK_ID: &str = "notification-task";

    fn fixture(
        context_events: u64,
        initialized: bool,
    ) -> (TempDir, Arc<Mutex<McpSessionState>>, u64) {
        let root = TempDir::new().unwrap();
        let mut registry = TaskRegistry::default();
        registry.tasks.insert(
            TASK_ID.to_string(),
            TaskRecord {
                task_id: TASK_ID.to_string(),
                ..TaskRecord::default()
            },
        );
        save_task_registry(root.path(), &registry).unwrap();

        let task_id = TaskStorageId::try_from(TASK_ID).unwrap();
        let event_path = task_event_log_path(root.path(), &task_id);
        std::fs::create_dir_all(event_path.parent().unwrap()).unwrap();
        let events = (1..=context_events)
            .map(|seq| {
                serde_json::to_string(&DaemonEventFrame {
                    seq,
                    task_id: TASK_ID.to_string(),
                    event: DaemonEvent {
                        kind: "context_updated".to_string(),
                        occurred_at_unix: seq,
                        data: json!({"context_version": format!("ctx-{seq}")}),
                    },
                })
                .unwrap()
            })
            .collect::<Vec<_>>()
            .join("\n");
        let events = if events.is_empty() {
            String::new()
        } else {
            format!("{events}\n")
        };
        std::fs::write(&event_path, events).unwrap();
        let event_log_len = std::fs::metadata(event_path).unwrap().len();

        let session = Arc::new(Mutex::new(McpSessionState {
            initialized,
            tracked_tasks: BTreeMap::from([(TASK_ID.to_string(), 0)]),
            tracked_task_offsets: BTreeMap::from([(TASK_ID.to_string(), 0)]),
            framing: Some(McpMessageFraming::NewlineJson),
            ..McpSessionState::default()
        }));
        (root, session, event_log_len)
    }

    fn start_deterministic_notification_task<Deliver, DeliveryFuture>(
        root: PathBuf,
        session: Arc<Mutex<McpSessionState>>,
        deliver: Deliver,
    ) -> NotificationTask
    where
        Deliver: FnMut(Value, McpMessageFraming) -> DeliveryFuture + Send + 'static,
        DeliveryFuture: Future<Output = Result<NotificationDelivery>> + Send + 'static,
    {
        start_notification_task_with_reader(
            root,
            session,
            super::super::MCP_NOTIFICATION_POLL_INTERVAL,
            |root, task_id, offset| async move {
                load_task_events_from_offset(&root, &task_id, offset)
                    .map(Some)
                    .map_err(anyhow::Error::from)
            },
            deliver,
        )
    }

    #[tokio::test(start_paused = true)]
    async fn idle_session_delivers_first_notification_within_one_interval_after_initialize() {
        let (root, session, _) = fixture(1, false);
        let (output, mut receiver) = proxy_output_channel();
        let notification_output = output.clone();
        let task = start_deterministic_notification_task(
            root.path().to_path_buf(),
            session.clone(),
            move |notification, framing| {
                let output = notification_output.clone();
                async move {
                    Ok(if output.try_send(notification, framing)? {
                        NotificationDelivery::Delivered
                    } else {
                        NotificationDelivery::Backpressured
                    })
                }
            },
        );
        tokio::task::yield_now().await;
        assert!(receiver.try_recv().is_err());

        session.lock().unwrap().initialized = true;
        tokio::time::advance(
            super::super::MCP_NOTIFICATION_POLL_INTERVAL - Duration::from_millis(1),
        )
        .await;
        tokio::task::yield_now().await;
        assert!(receiver.try_recv().is_err());

        tokio::time::advance(Duration::from_millis(1)).await;
        let message = receiver.recv().await.unwrap();
        assert_eq!(message.value["params"]["event_seq"], 1);
        task.shutdown().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn empty_event_log_remains_a_transient_no_event_read() {
        let (root, session, _) = fixture(0, true);
        let mut task = start_deterministic_notification_task(
            root.path().to_path_buf(),
            session.clone(),
            |_notification, _framing| async {
                panic!("empty event log must not deliver a notification")
            },
        );

        tokio::task::yield_now().await;

        assert_eq!(session.lock().unwrap().tracked_tasks[TASK_ID], 0);
        assert_eq!(session.lock().unwrap().tracked_task_offsets[TASK_ID], 0);
        task.request_shutdown();
        task.join().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn durable_event_corruption_stops_notifications_without_advancing_the_cursor() {
        use std::io::Write as _;

        let (root, session, _) = fixture(1, true);
        let task_id = TaskStorageId::try_from(TASK_ID).unwrap();
        let event_path = task_event_log_path(root.path(), &task_id);
        let mut events = std::fs::OpenOptions::new()
            .append(true)
            .open(event_path)
            .unwrap();
        events.write_all(b"{not-json}\n").unwrap();
        events.sync_all().unwrap();
        let mut task = start_deterministic_notification_task(
            root.path().to_path_buf(),
            session.clone(),
            |_notification, _framing| async {
                panic!("corrupt event log must not deliver a notification")
            },
        );

        let error = task.join().await.unwrap_err();

        assert!(error.to_string().contains("invalid task event frame"));
        assert_eq!(session.lock().unwrap().tracked_tasks[TASK_ID], 0);
        assert_eq!(session.lock().unwrap().tracked_task_offsets[TASK_ID], 0);
    }

    #[tokio::test(start_paused = true)]
    async fn saturated_output_replays_only_undelivered_notification_and_then_advances_offset() {
        let event_count = super::super::proxy_upstream::MAX_PROXY_OUTPUT_MESSAGES as u64 + 1;
        let (root, session, event_log_len) = fixture(event_count, true);
        let (output, mut receiver) = proxy_output_channel();
        let notification_output = output.clone();
        let task = start_deterministic_notification_task(
            root.path().to_path_buf(),
            session.clone(),
            move |notification, framing| {
                let output = notification_output.clone();
                async move {
                    Ok(if output.try_send(notification, framing)? {
                        NotificationDelivery::Delivered
                    } else {
                        NotificationDelivery::Backpressured
                    })
                }
            },
        );
        tokio::task::yield_now().await;

        let first_pass_seq = session.lock().unwrap().tracked_tasks[TASK_ID];
        assert_eq!(
            first_pass_seq,
            super::super::proxy_upstream::MAX_PROXY_OUTPUT_MESSAGES as u64
        );
        assert_eq!(session.lock().unwrap().tracked_task_offsets[TASK_ID], 0);
        let first_pass = std::iter::from_fn(|| receiver.try_recv().ok())
            .map(|message| message.value["params"]["event_seq"].as_u64().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            first_pass,
            (1..=super::super::proxy_upstream::MAX_PROXY_OUTPUT_MESSAGES as u64)
                .collect::<Vec<_>>()
        );

        tokio::time::advance(super::super::MCP_NOTIFICATION_POLL_INTERVAL).await;
        tokio::task::yield_now().await;
        let replay = receiver.try_recv().unwrap();
        assert_eq!(replay.value["params"]["event_seq"], event_count);
        assert_eq!(session.lock().unwrap().tracked_tasks[TASK_ID], event_count);
        assert_eq!(
            session.lock().unwrap().tracked_task_offsets[TASK_ID],
            event_log_len
        );
        task.shutdown().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_signal_joins_without_waiting_for_next_poll_interval() {
        let (root, session, _) = fixture(1, false);
        let started_at = tokio::time::Instant::now();
        let task = start_deterministic_notification_task(
            root.path().to_path_buf(),
            session,
            |_notification, _framing| async { Ok(NotificationDelivery::Delivered) },
        );
        tokio::task::yield_now().await;

        task.shutdown().await.unwrap();

        assert_eq!(tokio::time::Instant::now(), started_at);
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_cancels_a_backpressured_delivery_and_joins_immediately() {
        let (root, session, _) = fixture(1, true);
        let (entered_sender, entered_receiver) = tokio::sync::oneshot::channel();
        let mut entered_sender = Some(entered_sender);
        let started_at = tokio::time::Instant::now();
        let task = start_deterministic_notification_task(
            root.path().to_path_buf(),
            session,
            move |_notification, _framing| {
                let entered_sender = entered_sender.take();
                async move {
                    if let Some(entered_sender) = entered_sender {
                        let _ = entered_sender.send(());
                    }
                    std::future::pending().await
                }
            },
        );
        entered_receiver.await.unwrap();

        task.shutdown().await.unwrap();

        assert_eq!(tokio::time::Instant::now(), started_at);
    }

    #[test]
    fn notification_sources_have_no_unowned_os_thread_polling() {
        let forbidden = [
            ["std", "::thread"].concat(),
            ["thread", "::spawn"].concat(),
            ["thread", "::sleep"].concat(),
        ];
        for (name, source) in [
            ("cmd_mcp.rs", include_str!("cmd_mcp.rs")),
            ("cmd_mcp_proxy.rs", include_str!("cmd_mcp_proxy.rs")),
            (
                "cmd_mcp_notifications.rs",
                include_str!("cmd_mcp_notifications.rs"),
            ),
        ] {
            for pattern in &forbidden {
                assert!(
                    !source.contains(pattern),
                    "{name} contains unowned polling primitive {pattern}"
                );
            }
        }
    }
}
