use super::*;
use crate::broker::{
    broker_decompose, broker_estimate_context, broker_get_context, broker_prepare_handoff,
    broker_task_status, broker_validate_plan, broker_write_state, broker_write_state_batch,
    build_status, kernel_for_context_root, kernel_for_request, mark_handoff_consumed,
};
use crate::instruction_files::resolve_context;
use crate::runtime::{BlockingPool, DaemonRuntimeConfig};
use crate::state::TaskSubscriber;
use crate::watch::WatchIngress;
use packet28_daemon_protocol::frame::{FrameError, MAX_SOCKET_MESSAGE_BYTES};
use packet28_daemon_protocol::task::TaskMarkHandoffConsumedResponse;
use serde::Serialize;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

static NEXT_SUBSCRIBER_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
enum FrameReadError {
    Io(std::io::Error),
    Deadline(&'static str),
    Protocol(FrameError),
}

impl fmt::Display for FrameReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Deadline(phase) => {
                write!(formatter, "daemon frame {phase} read deadline exceeded")
            }
            Self::Protocol(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for FrameReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Protocol(error) => Some(error),
            Self::Deadline(_) => None,
        }
    }
}

pub(crate) async fn handle_connection<S>(
    state: Arc<Mutex<DaemonState>>,
    watch_tx: WatchIngress,
    mut stream: S,
    config: DaemonRuntimeConfig,
    blocking_pool: BlockingPool,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut shutdown = state.lock().map_err(lock_err)?.shutdown.subscribe();
    loop {
        let frame = tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
                continue;
            }
            frame = read_frame_bytes(&mut stream, &config) => frame,
        };
        let frame = match frame {
            Ok(frame) => frame,
            Err(error) if is_benign_disconnect_error(&error) => return Ok(()),
            Err(error) => {
                let response = DaemonResponse::Error {
                    message: error.to_string(),
                };
                write_async_frame(
                    &mut stream,
                    response,
                    config.frame_write_timeout,
                    &blocking_pool,
                )
                .await?;
                return Ok(());
            }
        };
        let request = match blocking_pool
            .run_control(move || {
                serde_json::from_slice::<DaemonRequest>(&frame)
                    .map_err(|error| anyhow!("invalid daemon request frame: {error}"))
            })
            .await
        {
            Ok(value) => value,
            Err(error) => {
                let response = DaemonResponse::Error {
                    message: error.to_string(),
                };
                write_async_frame(
                    &mut stream,
                    response,
                    config.frame_write_timeout,
                    &blocking_pool,
                )
                .await?;
                return Ok(());
            }
        };
        if matches!(&request, DaemonRequest::Stop) {
            write_control_frame(
                &mut stream,
                DaemonResponse::Ack {
                    message: "stopping".to_string(),
                },
                config.frame_write_timeout,
                &blocking_pool,
            )
            .await?;
            request_daemon_stop(&state, &blocking_pool)?;
            return Ok(());
        }
        if let DaemonRequest::TaskSubscribe {
            task_id,
            replay_last,
            after_seq,
        } = request
        {
            return handle_task_subscribe(
                state,
                &mut stream,
                task_id,
                replay_last,
                after_seq,
                &config,
                &blocking_pool,
            )
            .await;
        }

        let control_response = matches!(&request, DaemonRequest::TaskCancel { .. });
        let response =
            dispatch_request(state.clone(), watch_tx.clone(), request, &blocking_pool).await;
        let response = match response {
            Ok(value) => value,
            Err(error) => {
                let message = format!("{error:#}");
                daemon_log(&format!("daemon request failed: {message}"));
                DaemonResponse::Error { message }
            }
        };
        if control_response {
            write_control_frame(
                &mut stream,
                response,
                config.frame_write_timeout,
                &blocking_pool,
            )
            .await?;
        } else {
            write_async_frame(
                &mut stream,
                response,
                config.frame_write_timeout,
                &blocking_pool,
            )
            .await?;
        }
        if state.lock().map_err(lock_err)?.shutdown.is_requested() {
            return Ok(());
        }
    }
}

fn request_daemon_stop(
    state: &Arc<Mutex<DaemonState>>,
    blocking_pool: &BlockingPool,
) -> Result<()> {
    let (shutdown, index_result) = {
        let mut guard = state.lock().map_err(lock_err)?;
        guard.shutting_down = true;
        let index_result = guard.index_tx.send(IndexCommand::Shutdown);
        (guard.shutdown.clone(), index_result)
    };
    // The acknowledgement is already on the wire. This is the stop
    // linearization point: no later blocking request or child can be admitted.
    blocking_pool.request_shutdown();
    shutdown.request();
    index_result
}

async fn dispatch_request(
    state: Arc<Mutex<DaemonState>>,
    watch_tx: WatchIngress,
    request: DaemonRequest,
    blocking_pool: &BlockingPool,
) -> Result<DaemonResponse> {
    match request {
        DaemonRequest::TaskAwaitHandoff { request } => {
            let response = await_task_handoff(state, request, blocking_pool.clone()).await?;
            Ok(DaemonResponse::TaskAwaitHandoff { response })
        }
        DaemonRequest::TaskLaunchAgent { mut request } if request.wait_for_handoff => {
            let wait_request = launch_wait_request(&state, &request)?;
            let _ = await_task_handoff(state.clone(), wait_request, blocking_pool.clone()).await?;
            request.wait_for_handoff = false;
            run_blocking_request(
                state,
                watch_tx,
                DaemonRequest::TaskLaunchAgent { request },
                blocking_pool,
            )
            .await
        }
        request => run_blocking_request(state, watch_tx, request, blocking_pool).await,
    }
}

async fn run_blocking_request(
    state: Arc<Mutex<DaemonState>>,
    watch_tx: WatchIngress,
    request: DaemonRequest,
    blocking_pool: &BlockingPool,
) -> Result<DaemonResponse> {
    let request_state = state.clone();
    let response = if matches!(&request, DaemonRequest::TaskCancel { .. }) {
        blocking_pool
            .run_cancellation(move || handle_request_and_flush(request_state, watch_tx, request))
            .await?
    } else {
        blocking_pool
            .run(move || handle_request_and_flush(request_state, watch_tx, request))
            .await?
    };
    let changes = state.lock().map_err(lock_err)?.changes.clone();
    changes.notify();
    Ok(response)
}

fn handle_request_and_flush(
    state: Arc<Mutex<DaemonState>>,
    watch_tx: WatchIngress,
    request: DaemonRequest,
) -> Result<DaemonResponse> {
    let request_result = handle_request(state.clone(), watch_tx, request);
    let flush_result = flush_persistence(&state);
    match (request_result, flush_result) {
        (Ok(response), Ok(())) => Ok(response),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error.context("failed to checkpoint daemon request state")),
        (Err(error), Err(flush_error)) => Err(error.context(format!(
            "daemon request also failed to checkpoint state: {flush_error:#}"
        ))),
    }
}

fn launch_wait_request(
    state: &Arc<Mutex<DaemonState>>,
    request: &TaskLaunchAgentRequest,
) -> Result<TaskAwaitHandoffRequest> {
    let after_context_version = state
        .lock()
        .map_err(lock_err)?
        .tasks
        .tasks
        .get(&request.task_id)
        .and_then(|task| {
            task.latest_agent_bootstrap_mode
                .as_deref()
                .filter(|mode| *mode == "handoff")
                .and(task.latest_agent_context_version.clone())
        });
    Ok(TaskAwaitHandoffRequest {
        task_id: request.task_id.clone(),
        timeout_ms: request.handoff_timeout_ms,
        poll_ms: request.handoff_poll_ms,
        after_context_version,
    })
}

async fn await_task_handoff(
    state: Arc<Mutex<DaemonState>>,
    request: TaskAwaitHandoffRequest,
    blocking_pool: BlockingPool,
) -> Result<TaskAwaitHandoffResponse> {
    if request.task_id.trim().is_empty() {
        anyhow::bail!("daemon task await-handoff requires task_id");
    }
    let timeout = Duration::from_millis(request.timeout_ms.unwrap_or(300_000));
    let poll = Duration::from_millis(request.poll_ms.unwrap_or(250).max(10));
    let started = tokio::time::Instant::now();
    let deadline = started + timeout;
    let (mut changes, mut shutdown) = {
        let guard = state.lock().map_err(lock_err)?;
        (guard.changes.subscribe(), guard.shutdown.subscribe())
    };
    let mut polls = 0_u64;
    loop {
        polls = polls.saturating_add(1);
        let status_state = state.clone();
        let task_id = request.task_id.clone();
        let status = blocking_pool
            .run(move || broker_task_status(status_state, BrokerTaskStatusRequest { task_id }))
            .await?;
        let is_newer_than_after = request
            .after_context_version
            .as_ref()
            .is_none_or(|after| status.latest_context_version.as_deref() != Some(after.as_str()));
        if status.handoff_ready && is_newer_than_after {
            return Ok(TaskAwaitHandoffResponse {
                task_status: status,
                waited_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                polls,
            });
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(handoff_timeout_error(&request, status));
        }
        let next_poll = (tokio::time::Instant::now() + poll).min(deadline);
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    anyhow::bail!(
                        "daemon stopped while waiting for Packet28 handoff for task '{}'",
                        request.task_id
                    );
                }
            }
            changed = changes.changed() => {
                if changed.is_err() {
                    tokio::time::sleep_until(next_poll).await;
                }
            }
            () = tokio::time::sleep_until(next_poll) => {}
        }
    }
}

fn handoff_timeout_error(
    request: &TaskAwaitHandoffRequest,
    status: BrokerTaskStatusResponse,
) -> anyhow::Error {
    let waiting_for_newer_context = request
        .after_context_version
        .as_ref()
        .is_some_and(|after| status.latest_context_version.as_deref() == Some(after.as_str()));
    let reason = if waiting_for_newer_context {
        request
            .after_context_version
            .as_ref()
            .map(|after| {
                format!("newer handoff than context version '{after}' did not become ready")
            })
            .unwrap_or_else(|| "handoff did not become ready".to_string())
    } else {
        status
            .handoff_reason
            .unwrap_or_else(|| "handoff did not become ready".to_string())
    };
    anyhow!(
        "timed out waiting for Packet28 handoff for task '{}': {}",
        request.task_id,
        reason
    )
}

struct SubscriberRegistration {
    state: Arc<Mutex<DaemonState>>,
    task_id: String,
    subscriber_id: u64,
}

impl Drop for SubscriberRegistration {
    fn drop(&mut self) {
        let Ok(mut guard) = self.state.lock() else {
            return;
        };
        if let Some(subscribers) = guard.subscribers.get_mut(&self.task_id) {
            subscribers.retain(|subscriber| subscriber.id != self.subscriber_id);
            if subscribers.is_empty() {
                guard.subscribers.remove(&self.task_id);
            }
        }
    }
}

async fn handle_task_subscribe<S>(
    state: Arc<Mutex<DaemonState>>,
    stream: &mut S,
    task_id: String,
    replay_last: usize,
    after_seq: Option<u64>,
    config: &DaemonRuntimeConfig,
    blocking_pool: &BlockingPool,
) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    let subscriber_id = NEXT_SUBSCRIBER_ID.fetch_add(1, Ordering::Relaxed);
    let (sender, mut receiver) = tokio::sync::mpsc::channel(config.subscriber_queue_capacity);
    let (root, snapshot_seq, mut shutdown) = {
        let mut guard = state.lock().map_err(lock_err)?;
        let snapshot_seq = guard
            .tasks
            .tasks
            .get(&task_id)
            .map_or(0, |task| task.last_event_seq);
        guard
            .subscribers
            .entry(task_id.clone())
            .or_default()
            .push(TaskSubscriber {
                id: subscriber_id,
                sender,
            });
        (guard.root.clone(), snapshot_seq, guard.shutdown.subscribe())
    };
    let _registration = SubscriberRegistration {
        state,
        task_id: task_id.clone(),
        subscriber_id,
    };
    let replay_task_id = task_id.clone();
    let replay = blocking_pool
        .run(move || Ok(load_task_events(&root, &replay_task_id)?))
        .await?;
    let replay = select_replay_events(
        replay
            .into_iter()
            .filter(|frame| frame.seq <= snapshot_seq)
            .collect(),
        replay_last,
        after_seq,
    );
    write_async_frame(
        stream,
        DaemonResponse::TaskSubscribeAck {
            task_id: task_id.clone(),
            replayed: replay.len(),
            after_seq,
        },
        config.frame_write_timeout,
        blocking_pool,
    )
    .await?;
    for frame in replay {
        write_async_frame(stream, frame, config.frame_write_timeout, blocking_pool).await?;
    }

    loop {
        let frame = tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
                continue;
            }
            frame = receiver.recv() => frame,
        };
        let Some(frame) = frame else {
            // A full bounded queue removes the sender. Closing the stream after
            // draining makes lag visible; the client's last sequence is then
            // an exact replay cursor for the next subscription.
            break;
        };
        write_async_frame(stream, frame, config.frame_write_timeout, blocking_pool).await?;
    }
    Ok(())
}

pub(crate) fn select_replay_events(
    events: Vec<DaemonEventFrame>,
    replay_last: usize,
    after_seq: Option<u64>,
) -> Vec<DaemonEventFrame> {
    let replay: Vec<_> = match after_seq {
        Some(after_seq) => events
            .into_iter()
            .filter(|frame| frame.seq > after_seq)
            .collect(),
        None => events,
    };
    if replay_last == 0 || replay_last >= replay.len() {
        replay
    } else {
        replay[replay.len().saturating_sub(replay_last)..].to_vec()
    }
}

async fn read_frame_bytes<R>(
    reader: &mut R,
    config: &DaemonRuntimeConfig,
) -> std::result::Result<Vec<u8>, FrameReadError>
where
    R: AsyncRead + Unpin,
{
    let mut len_bytes = [0_u8; 8];
    read_exact_with_deadline(
        reader,
        &mut len_bytes,
        config.frame_header_timeout,
        "header",
    )
    .await?;
    let declared = u64::from_be_bytes(len_bytes);
    let len = usize::try_from(declared)
        .map_err(|_| FrameReadError::Protocol(FrameError::LengthOverflow))?;
    if len == 0 {
        return Err(FrameReadError::Protocol(FrameError::Empty));
    }
    if len > MAX_SOCKET_MESSAGE_BYTES {
        return Err(FrameReadError::Protocol(FrameError::TooLarge {
            actual: declared,
            limit: MAX_SOCKET_MESSAGE_BYTES,
        }));
    }
    let mut body = vec![0_u8; len];
    read_exact_with_deadline(reader, &mut body, config.frame_body_timeout, "body").await?;
    Ok(body)
}

async fn read_exact_with_deadline<R>(
    reader: &mut R,
    bytes: &mut [u8],
    deadline: Duration,
    phase: &'static str,
) -> std::result::Result<(), FrameReadError>
where
    R: AsyncRead + Unpin,
{
    tokio::time::timeout(deadline, reader.read_exact(bytes))
        .await
        .map_err(|_| FrameReadError::Deadline(phase))?
        .map(|_| ())
        .map_err(FrameReadError::Io)
}

async fn write_async_frame<W, T>(
    writer: &mut W,
    value: T,
    deadline: Duration,
    blocking_pool: &BlockingPool,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize + Send + 'static,
{
    write_async_frame_with_lane(writer, value, deadline, blocking_pool, false).await
}

async fn write_control_frame<W, T>(
    writer: &mut W,
    value: T,
    deadline: Duration,
    blocking_pool: &BlockingPool,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize + Send + 'static,
{
    write_async_frame_with_lane(writer, value, deadline, blocking_pool, true).await
}

async fn write_async_frame_with_lane<W, T>(
    writer: &mut W,
    value: T,
    deadline: Duration,
    blocking_pool: &BlockingPool,
    control: bool,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize + Send + 'static,
{
    let encode = move || {
        let mut encoded = Vec::new();
        write_frame(&mut encoded, &value)?;
        Ok(encoded)
    };
    let encoded = if control {
        blocking_pool.run_control(encode).await?
    } else {
        blocking_pool.run(encode).await?
    };
    tokio::time::timeout(deadline, async {
        writer.write_all(&encoded).await?;
        writer.flush().await
    })
    .await
    .map_err(|_| anyhow!("daemon frame write deadline exceeded"))??;
    Ok(())
}

fn is_benign_disconnect_error(error: &FrameReadError) -> bool {
    matches!(
        error,
        FrameReadError::Io(io_error)
            if matches!(
                io_error.kind(),
                ErrorKind::BrokenPipe
                    | ErrorKind::ConnectionReset
                    | ErrorKind::UnexpectedEof
            )
    )
}

fn handle_request(
    state: Arc<Mutex<DaemonState>>,
    watch_tx: WatchIngress,
    request: DaemonRequest,
) -> Result<DaemonResponse> {
    match request {
        DaemonRequest::Execute { request } => {
            let kernel = kernel_for_request(&state, &request)?;
            let response = kernel.execute(request)?;
            Ok(DaemonResponse::Execute { response })
        }
        DaemonRequest::ExecuteSequence { spec } => {
            let (task, watches) = register_task_and_watches(state.clone(), watch_tx, spec)?;
            let response = match run_sequence_for_task(state.clone(), &task.task_id) {
                Ok(response) => response,
                Err(err) => {
                    daemon_log(&format!(
                        "initial task run failed task_id={} error={err}",
                        task.task_id
                    ));
                    let _ = cancel_task(state.clone(), &task.task_id);
                    return Err(err);
                }
            };
            if let Some(failure) = response
                .step_results
                .iter()
                .find_map(|step| step.failure.as_ref())
            {
                let message = failure.message.clone();
                daemon_log(&format!(
                    "initial task run failed task_id={} error={message}",
                    task.task_id
                ));
                let _ = cancel_task(state.clone(), &task.task_id);
                return Err(anyhow!(message));
            }
            let task = state
                .lock()
                .map_err(lock_err)?
                .tasks
                .tasks
                .get(&task.task_id)
                .cloned()
                .unwrap_or(task);
            Ok(DaemonResponse::ExecuteSequence {
                response,
                task,
                watches,
            })
        }
        DaemonRequest::Status => {
            let guard = state.lock().map_err(lock_err)?;
            let status = build_status(&guard)?;
            Ok(DaemonResponse::Status { status })
        }
        DaemonRequest::Stop => Err(anyhow!(
            "daemon stop must be acknowledged by the async control plane"
        )),
        DaemonRequest::TaskStatus { task_id } => {
            let task = state
                .lock()
                .map_err(lock_err)?
                .tasks
                .tasks
                .get(&task_id)
                .cloned();
            Ok(DaemonResponse::TaskStatus { task })
        }
        DaemonRequest::TaskAwaitHandoff { .. } => Err(anyhow!(
            "task await-handoff must run on the daemon async orchestration boundary"
        )),
        DaemonRequest::TaskMarkHandoffConsumed { request } => {
            let response = TaskMarkHandoffConsumedResponse {
                handoff: mark_handoff_consumed(&state, &request.task_id, &request.handoff_id)?,
            };
            Ok(DaemonResponse::TaskMarkHandoffConsumed { response })
        }
        DaemonRequest::TaskLaunchAgent { request } => {
            let response = task_launch_agent(state, request)?;
            Ok(DaemonResponse::TaskLaunchAgent { response })
        }
        DaemonRequest::TaskCancel { task_id } => {
            let cancellation = cancel_task(state.clone(), &task_id)?;
            Ok(DaemonResponse::TaskCancel {
                task: cancellation.0,
                removed_watch_ids: cancellation.1,
            })
        }
        DaemonRequest::TaskSubscribe { .. } => {
            Err(anyhow!("task subscribe is handled as a streaming request"))
        }
        DaemonRequest::WatchList { task_id } => {
            let state = state.lock().map_err(lock_err)?;
            let watches = state
                .watches
                .watches
                .iter()
                .filter(|watch| {
                    task_id
                        .as_ref()
                        .map(|task_id| watch.spec.task_id == *task_id)
                        .unwrap_or(true)
                })
                .cloned()
                .collect();
            Ok(DaemonResponse::WatchList { watches })
        }
        DaemonRequest::WatchRemove { watch_id } => {
            let removed = remove_watch(state, &watch_id)?;
            Ok(DaemonResponse::WatchRemove { removed })
        }
        DaemonRequest::PacketFetch { request } => {
            let root = resolve_root(Path::new(&request.root));
            let value = suite_packet_core::read_packet_artifact(&root, &request.handle)
                .map_err(|source| anyhow!(source.to_string()))?;
            let wrapper = serde_json::from_value(value)
                .map_err(|source| anyhow!("invalid packet artifact: {source}"))?;
            Ok(DaemonResponse::PacketFetch {
                response: PacketFetchResponse { wrapper },
            })
        }
        DaemonRequest::CoverCheck { request } => {
            let response = run_cover_check(request)?;
            Ok(DaemonResponse::CoverCheck { response })
        }
        DaemonRequest::TestShard { request } => {
            let response = run_test_shard(request)?;
            Ok(DaemonResponse::TestShard { response })
        }
        DaemonRequest::TestMap { request } => {
            let response = run_test_map(request)?;
            Ok(DaemonResponse::TestMap { response })
        }
        DaemonRequest::ContextStoreList { request } => {
            let kernel = kernel_for_context_root(&state, &request.root)?;
            let response = run_context_store_list(&kernel, request)?;
            Ok(DaemonResponse::ContextStoreList { response })
        }
        DaemonRequest::ContextStoreGet { request } => {
            let kernel = kernel_for_context_root(&state, &request.root)?;
            let response = run_context_store_get(&kernel, request)?;
            Ok(DaemonResponse::ContextStoreGet { response })
        }
        DaemonRequest::ContextStorePrune { request } => {
            let kernel = kernel_for_context_root(&state, &request.root)?;
            let response = run_context_store_prune(&kernel, request)?;
            Ok(DaemonResponse::ContextStorePrune { response })
        }
        DaemonRequest::ContextStoreStats { request } => {
            let kernel = kernel_for_context_root(&state, &request.root)?;
            let response = run_context_store_stats(&kernel, request)?;
            Ok(DaemonResponse::ContextStoreStats { response })
        }
        DaemonRequest::ContextRecall { request } => {
            let kernel = kernel_for_context_root(&state, &request.root)?;
            let response = run_context_recall(&kernel, request)?;
            Ok(DaemonResponse::ContextRecall { response })
        }
        DaemonRequest::BrokerGetContext { request } => {
            let response = broker_get_context(state, request)?;
            Ok(DaemonResponse::BrokerGetContext { response })
        }
        DaemonRequest::BrokerEstimateContext { request } => {
            let response = broker_estimate_context(state, request)?;
            Ok(DaemonResponse::BrokerEstimateContext { response })
        }
        DaemonRequest::BrokerPrepareHandoff { request } => {
            let response = broker_prepare_handoff(state, request)?;
            Ok(DaemonResponse::BrokerPrepareHandoff { response })
        }
        DaemonRequest::BrokerValidatePlan { request } => {
            let response = broker_validate_plan(state, request)?;
            Ok(DaemonResponse::BrokerValidatePlan { response })
        }
        DaemonRequest::BrokerDecompose { request } => {
            let response = broker_decompose(state, request)?;
            Ok(DaemonResponse::BrokerDecompose { response })
        }
        DaemonRequest::BrokerWriteState { request } => {
            let response = broker_write_state(state, request)?;
            Ok(DaemonResponse::BrokerWriteState { response })
        }
        DaemonRequest::BrokerWriteStateBatch { request } => {
            let response = broker_write_state_batch(state, request)?;
            Ok(DaemonResponse::BrokerWriteStateBatch { response })
        }
        DaemonRequest::BrokerTaskStatus { request } => {
            let response = broker_task_status(state, request)?;
            Ok(DaemonResponse::BrokerTaskStatus { response })
        }
        DaemonRequest::ContextResolve { request } => {
            let response = resolve_context(state, request)?;
            Ok(DaemonResponse::ContextResolve { response })
        }
        DaemonRequest::InstructionFileResolve { request } => {
            let response = resolve_instruction_file(state, request)?;
            Ok(DaemonResponse::InstructionFileResolve { response })
        }
        DaemonRequest::HookIngest { request } => {
            let response = hook_ingest(state, request)?;
            Ok(DaemonResponse::HookIngest { response })
        }
        DaemonRequest::Packet28Search { request } => {
            let response = daemon_packet28_search(state, request)?;
            Ok(DaemonResponse::Packet28Search { response })
        }
        DaemonRequest::Packet28SearchGuard { request } => {
            let response = crate::index::daemon_packet28_search_guard(state, request)?;
            Ok(DaemonResponse::Packet28SearchGuard { response })
        }
        DaemonRequest::DaemonIndexStatus { request: _ } => {
            let response = daemon_index_status(state)?;
            Ok(DaemonResponse::DaemonIndexStatus { response })
        }
        DaemonRequest::DaemonIndexRebuild { request } => {
            let response = daemon_index_rebuild(state, request)?;
            Ok(DaemonResponse::DaemonIndexRebuild { response })
        }
        DaemonRequest::DaemonIndexClear { request: _ } => {
            let response = daemon_index_clear(state)?;
            Ok(DaemonResponse::DaemonIndexClear { response })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTROL_LANE_DEADLOCK_TIMEOUT: Duration = Duration::from_secs(10);

    async fn exchange(
        stream: &mut tokio::io::DuplexStream,
        request: &DaemonRequest,
    ) -> DaemonResponse {
        let mut encoded = Vec::new();
        write_frame(&mut encoded, request).unwrap();
        stream.write_all(&encoded).await.unwrap();
        let mut header = [0_u8; 8];
        stream.read_exact(&mut header).await.unwrap();
        let length = usize::try_from(u64::from_be_bytes(header)).unwrap();
        let mut body = vec![0_u8; length];
        stream.read_exact(&mut body).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn frame(seq: u64) -> DaemonEventFrame {
        DaemonEventFrame {
            seq,
            task_id: "task-1".to_string(),
            event: DaemonEvent {
                kind: "test".to_string(),
                occurred_at_unix: seq,
                data: json!({ "seq": seq }),
            },
        }
    }

    #[test]
    fn replay_after_seq_returns_only_later_frames() {
        let selected = select_replay_events(vec![frame(1), frame(2), frame(3)], 0, Some(1));
        assert_eq!(
            selected.iter().map(|frame| frame.seq).collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn replay_last_limits_frames_after_cursor() {
        let selected =
            select_replay_events(vec![frame(1), frame(2), frame(3), frame(4)], 2, Some(1));
        assert_eq!(
            selected.iter().map(|frame| frame.seq).collect::<Vec<_>>(),
            vec![3, 4]
        );
    }

    fn deadline_config() -> DaemonRuntimeConfig {
        DaemonRuntimeConfig {
            frame_header_timeout: Duration::from_secs(1),
            frame_body_timeout: Duration::from_secs(1),
            frame_write_timeout: Duration::from_secs(1),
            ..DaemonRuntimeConfig::default()
        }
    }

    fn fresh_hook_artifact_packet() -> packet28_daemon_protocol::hooks::HookReducerPacket {
        packet28_daemon_protocol::hooks::HookReducerPacket {
            packet_type: "packet28.hook.fs.v2".to_string(),
            tool_name: "Read".to_string(),
            operation_kind: suite_packet_core::ToolOperationKind::Read,
            summary: "fresh task artifact".to_string(),
            cacheable: Some(false),
            mutation: Some(false),
            artifact: Some(json!({"source": "fresh-task-admission-regression"})),
            ..packet28_daemon_protocol::hooks::HookReducerPacket::default()
        }
    }

    #[test]
    fn fresh_hook_artifact_request_admits_registry_before_namespace_and_end_flush() {
        let state = crate::tests::support::daemon_test_state_with_persistence_debounce(
            Duration::from_secs(1),
        );
        let root = crate::tests::support::daemon_test_root(&state);
        let task_id = "fresh-hook-artifact";
        let (watch_tx, _watch_rx) = WatchIngress::new(1);

        let response = handle_request_and_flush(
            state.clone(),
            watch_tx,
            DaemonRequest::HookIngest {
                request: packet28_daemon_protocol::hooks::HookIngestRequest {
                    task_id: task_id.to_string(),
                    event_kind: packet28_daemon_protocol::hooks::HookEventKind::PostToolUse,
                    reducer_packet: Some(fresh_hook_artifact_packet()),
                    ..packet28_daemon_protocol::hooks::HookIngestRequest::default()
                },
            },
        )
        .unwrap();
        assert!(matches!(
            response,
            DaemonResponse::HookIngest {
                response: packet28_daemon_protocol::hooks::HookIngestResponse {
                    accepted: true,
                    ..
                }
            }
        ));

        let registry = packet28_daemon_core::storage::load_task_registry(&root).unwrap();
        assert!(registry.tasks.contains_key(task_id));
        let storage_id = task_storage_id(task_id).unwrap();
        let hook_artifacts = task_artifact_dir(&root, &storage_id).join("hook-artifacts");
        assert_eq!(std::fs::read_dir(hook_artifacts).unwrap().count(), 1);
        crate::tests::support::shutdown_test_persistence(&state);
    }

    #[test]
    fn fresh_broker_artifact_request_admits_registry_before_namespace_and_end_flush() {
        let state = crate::tests::support::daemon_test_state_with_persistence_debounce(
            Duration::from_secs(1),
        );
        let root = crate::tests::support::daemon_test_root(&state);
        let task_id = "fresh-broker-artifact";
        let (watch_tx, _watch_rx) = WatchIngress::new(1);

        let response = handle_request_and_flush(
            state.clone(),
            watch_tx,
            DaemonRequest::BrokerGetContext {
                request: BrokerGetContextRequest {
                    task_id: task_id.to_string(),
                    action: Some(BrokerAction::Inspect),
                    max_sections: Some(1),
                    persist_artifacts: Some(true),
                    ..BrokerGetContextRequest::default()
                },
            },
        )
        .unwrap();
        assert!(matches!(
            response,
            DaemonResponse::BrokerGetContext {
                response: BrokerGetContextResponse {
                    artifact_id: Some(_),
                    ..
                }
            }
        ));

        let registry = packet28_daemon_core::storage::load_task_registry(&root).unwrap();
        assert!(registry.tasks.contains_key(task_id));
        let storage_id = task_storage_id(task_id).unwrap();
        assert!(task_brief_markdown_path(&root, &storage_id).is_file());
        assert!(task_brief_json_path(&root, &storage_id).is_file());
        assert!(task_state_json_path(&root, &storage_id).is_file());
        crate::tests::support::shutdown_test_persistence(&state);
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_frame_header_hits_its_owned_deadline() {
        let (mut client, mut server) = tokio::io::duplex(64);
        client.write_all(&[0_u8; 4]).await.unwrap();

        let error = read_frame_bytes(&mut server, &deadline_config())
            .await
            .unwrap_err();
        assert!(matches!(error, FrameReadError::Deadline("header")));
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_frame_body_hits_its_owned_deadline() {
        let (mut client, mut server) = tokio::io::duplex(64);
        client.write_all(&2_u64.to_be_bytes()).await.unwrap();
        client.write_all(b"{").await.unwrap();

        let error = read_frame_bytes(&mut server, &deadline_config())
            .await
            .unwrap_err();
        assert!(matches!(error, FrameReadError::Deadline("body")));
    }

    #[tokio::test(start_paused = true)]
    async fn non_reading_peer_hits_frame_write_deadline() {
        let (_client, mut server) = tokio::io::duplex(32);
        let response = DaemonResponse::Error {
            message: "x".repeat(64 * 1_024),
        };
        let error = write_async_frame(
            &mut server,
            response,
            Duration::from_secs(1),
            &BlockingPool::new(1),
        )
        .await
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("daemon frame write deadline exceeded"));
    }

    #[tokio::test(start_paused = true)]
    async fn handoff_wait_uses_owned_timer_deadline() {
        let state = crate::tests::support::daemon_test_state();
        let error = await_task_handoff(
            state,
            TaskAwaitHandoffRequest {
                task_id: "task-await-timeout".to_string(),
                timeout_ms: Some(100),
                poll_ms: Some(10),
                after_context_version: None,
            },
            BlockingPool::new(1),
        )
        .await
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("timed out waiting for Packet28 handoff"));
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_signal_cancels_handoff_wait() {
        let state = crate::tests::support::daemon_test_state();
        let wait_state = state.clone();
        let waiter = tokio::spawn(async move {
            await_task_handoff(
                wait_state,
                TaskAwaitHandoffRequest {
                    task_id: "task-await-cancel".to_string(),
                    timeout_ms: Some(60_000),
                    poll_ms: Some(10_000),
                    after_context_version: None,
                },
                BlockingPool::new(1),
            )
            .await
        });
        tokio::task::yield_now().await;
        state.lock().unwrap().shutdown.request();

        let error = waiter.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("daemon stopped while waiting"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_ack_uses_reserved_control_lane_before_publishing_shutdown() {
        let state = crate::tests::support::daemon_test_state();
        let pool = BlockingPool::new(1);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let worker_pool = pool.clone();
        let data_worker = tokio::spawn(async move {
            worker_pool
                .run(move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .await
        });
        started_rx.await.unwrap();
        assert_eq!(pool.available_permits(), 0);
        assert_eq!(
            pool.available_control_permits(),
            crate::runtime::CONTROL_BLOCKING_OPERATIONS
        );

        let (mut client, server_stream) = tokio::io::duplex(4 * 1_024);
        let (watch_tx, _watch_rx) = WatchIngress::new(1);
        let server_state = state.clone();
        let server_pool = pool.clone();
        let server = tokio::spawn(async move {
            handle_connection(
                server_state,
                watch_tx,
                server_stream,
                deadline_config(),
                server_pool,
            )
            .await
        });

        let response = tokio::time::timeout(
            CONTROL_LANE_DEADLOCK_TIMEOUT,
            exchange(&mut client, &DaemonRequest::Stop),
        )
        .await
        .expect("Stop deadlocked behind saturated data work");
        assert!(matches!(
            response,
            DaemonResponse::Ack { ref message } if message == "stopping"
        ));
        assert!(state.lock().unwrap().shutdown.is_requested());
        assert!(pool.is_shutting_down());
        server.await.unwrap().unwrap();

        release_tx.send(()).unwrap();
        data_worker.await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn task_cancel_uses_reserved_control_lane_when_data_is_saturated() {
        let state = crate::tests::support::daemon_test_state();
        crate::tests::support::insert_admitted_task_record(
            &state,
            TaskRecord {
                task_id: "task-control-cancel".to_string(),
                ..TaskRecord::default()
            },
        );
        flush_persistence(&state).unwrap();
        {
            let mut guard = state.lock().unwrap();
            guard
                .task_generations
                .create("task-control-cancel")
                .unwrap();
        }
        let pool = BlockingPool::new(1);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let worker_pool = pool.clone();
        let data_worker = tokio::spawn(async move {
            worker_pool
                .run(move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .await
        });
        started_rx.await.unwrap();

        let (mut client, server_stream) = tokio::io::duplex(4 * 1_024);
        let (watch_tx, _watch_rx) = WatchIngress::new(1);
        let server_state = state.clone();
        let server_pool = pool.clone();
        let server = tokio::spawn(async move {
            handle_connection(
                server_state,
                watch_tx,
                server_stream,
                deadline_config(),
                server_pool,
            )
            .await
        });
        let response = tokio::time::timeout(
            CONTROL_LANE_DEADLOCK_TIMEOUT,
            exchange(
                &mut client,
                &DaemonRequest::TaskCancel {
                    task_id: "task-control-cancel".to_string(),
                },
            ),
        )
        .await
        .expect("TaskCancel deadlocked behind saturated data work");
        assert!(matches!(
            response,
            DaemonResponse::TaskCancel { task: Some(_), .. }
        ));

        state.lock().unwrap().shutdown.request();
        server.await.unwrap().unwrap();
        release_tx.send(()).unwrap();
        data_worker.await.unwrap().unwrap();
    }

    #[test]
    fn context_store_handlers_observe_and_prune_the_live_kernel_owner() {
        let state = crate::tests::support::daemon_test_state();
        let root = state.lock().unwrap().root.to_string_lossy().to_string();
        let (watch_tx, _watch_rx) = WatchIngress::new(1);
        let execute = handle_request(
            state.clone(),
            watch_tx.clone(),
            DaemonRequest::Execute {
                request: KernelRequest {
                    target: "contextq.assemble".to_string(),
                    input_packets: vec![context_kernel_core::KernelPacket::from_value(
                        json!({
                            "packet_id": "live-context",
                            "tool": "packet28d",
                            "reducer": "context",
                            "sections": [{
                                "title": "Live context",
                                "body": "daemon immediate visibility marker",
                                "refs": [],
                                "relevance": 1.0
                            }]
                        }),
                        None,
                    )],
                    ..KernelRequest::default()
                },
            },
        )
        .unwrap();
        assert!(matches!(execute, DaemonResponse::Execute { .. }));

        let listed = handle_request(
            state.clone(),
            watch_tx.clone(),
            DaemonRequest::ContextStoreList {
                request: ContextStoreListRequest {
                    root: root.clone(),
                    limit: 20,
                    ..ContextStoreListRequest::default()
                },
            },
        )
        .unwrap();
        let entries = match listed {
            DaemonResponse::ContextStoreList { response } => response.entries,
            response => panic!("unexpected list response: {response:?}"),
        };
        assert_eq!(entries.len(), 1);

        let fetched = handle_request(
            state.clone(),
            watch_tx.clone(),
            DaemonRequest::ContextStoreGet {
                request: ContextStoreGetRequest {
                    root: root.clone(),
                    key: entries[0].cache_key.clone(),
                },
            },
        )
        .unwrap();
        assert!(matches!(
            fetched,
            DaemonResponse::ContextStoreGet {
                response: ContextStoreGetResponse { entry: Some(_) }
            }
        ));

        let recalled = handle_request(
            state.clone(),
            watch_tx.clone(),
            DaemonRequest::ContextRecall {
                request: ContextRecallRequest {
                    query: "immediate visibility".to_string(),
                    root: root.clone(),
                    limit: 10,
                    since: Some(0),
                    ..ContextRecallRequest::default()
                },
            },
        )
        .unwrap();
        assert!(matches!(
            recalled,
            DaemonResponse::ContextRecall {
                response: ContextRecallResponse { ref hits, .. }
            } if hits.len() == 1
        ));

        let legacy_default_root_stats = handle_request(
            state.clone(),
            watch_tx.clone(),
            DaemonRequest::ContextStoreStats {
                request: ContextStoreStatsRequest {
                    root: String::new(),
                },
            },
        )
        .unwrap();
        assert!(matches!(
            legacy_default_root_stats,
            DaemonResponse::ContextStoreStats {
                response: ContextStoreStatsResponse {
                    stats: context_memory_core::ContextStoreStats { entries: 1, .. }
                }
            }
        ));

        let pruned = handle_request(
            state.clone(),
            watch_tx.clone(),
            DaemonRequest::ContextStorePrune {
                request: ContextStorePruneDaemonRequest {
                    root: root.clone(),
                    all: true,
                    ttl_secs: None,
                },
            },
        )
        .unwrap();
        assert!(matches!(
            pruned,
            DaemonResponse::ContextStorePrune {
                response: ContextStorePruneResponse {
                    report: context_memory_core::ContextStorePruneReport {
                        removed: 1,
                        remaining: 0,
                        ..
                    }
                }
            }
        ));

        let stats = handle_request(
            state,
            watch_tx,
            DaemonRequest::ContextStoreStats {
                request: ContextStoreStatsRequest { root },
            },
        )
        .unwrap();
        assert!(matches!(
            stats,
            DaemonResponse::ContextStoreStats {
                response: ContextStoreStatsResponse {
                    stats: context_memory_core::ContextStoreStats { entries: 0, .. }
                }
            }
        ));
    }
}
