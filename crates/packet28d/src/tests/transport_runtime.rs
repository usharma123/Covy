use super::support::{daemon_test_state, insert_admitted_task_record};
use super::*;
use fs2::FileExt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

static TCP_TRANSPORT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn shutdown_waiter(signal: ShutdownSignal) -> tokio::task::JoinHandle<Result<()>> {
    tokio::spawn(async move {
        let mut receiver = signal.subscribe();
        while !*receiver.borrow() {
            receiver
                .changed()
                .await
                .map_err(|_| anyhow!("test shutdown signal closed"))?;
        }
        Ok(())
    })
}

async fn exchange<S>(stream: &mut S, request: &DaemonRequest) -> DaemonResponse
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut encoded = Vec::new();
    write_frame(&mut encoded, request).unwrap();
    stream.write_all(&encoded).await.unwrap();

    let mut header = [0_u8; 8];
    stream.read_exact(&mut header).await.unwrap();
    let len = usize::try_from(u64::from_be_bytes(header)).unwrap();
    let mut body = vec![0_u8; len];
    stream.read_exact(&mut body).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn processor_failure_is_fatal_bounded_and_detached_work_retains_its_lease() {
    let state = daemon_test_state();
    let root = state.lock().unwrap().root.clone();
    let daemon_instance_lease =
        packet28_daemon_core::task_store_lease::acquire_daemon_instance_lease(&root).unwrap();
    let (_, task_store_lease) =
        packet28_daemon_core::retention::recover_task_store_quarantine_and_acquire_daemon_lease(
            &root,
            &daemon_instance_lease,
        )
        .unwrap();
    let shutdown = state.lock().unwrap().shutdown.clone();
    let blocking_pool = BlockingPool::with_lifecycle_leases(
        1,
        daemon_instance_lease.clone(),
        task_store_lease.clone(),
    );
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let (index_started_tx, index_started_rx) = tokio::sync::oneshot::channel();
    let (index_release_tx, index_release_rx) = std::sync::mpsc::sync_channel(1);
    let worker_pool = blocking_pool.clone();
    let mutation_state = state.clone();
    let blocking_worker = tokio::spawn(async move {
        worker_pool
            .run(move || {
                started_tx
                    .send(())
                    .map_err(|_| anyhow!("start probe closed"))?;
                release_rx
                    .recv()
                    .map_err(|_| anyhow!("release probe closed"))?;
                let mut guard = mutation_state.lock().map_err(lock_err)?;
                guard.tasks.tasks.insert(
                    "task-during-shutdown".to_string(),
                    TaskRecord {
                        task_id: "task-during-shutdown".to_string(),
                        ..TaskRecord::default()
                    },
                );
                persist_state(&guard)?;
                Ok(())
            })
            .await
    });
    started_rx.await.unwrap();
    let index_daemon_instance_lease = daemon_instance_lease.clone();
    let index_task_store_lease = task_store_lease.clone();
    let index_worker = tokio::task::spawn_blocking(move || {
        let _daemon_instance_lease = index_daemon_instance_lease;
        let _task_store_lease = index_task_store_lease;
        index_started_tx
            .send(())
            .map_err(|_| anyhow!("index start probe closed"))?;
        index_release_rx
            .recv()
            .map_err(|_| anyhow!("index release probe closed"))?;
        Ok(())
    });
    index_started_rx.await.unwrap();
    std::fs::write(ready_path(&root), b"ready\n").unwrap();

    let supervisor_started = Instant::now();
    let supervisor = tokio::spawn(supervise_daemon_tasks(
        state.clone(),
        shutdown.clone(),
        blocking_pool,
        Duration::from_millis(25),
        DaemonRuntimeTasks {
            transport: shutdown_waiter(shutdown.clone()),
            watch: tokio::spawn(async { Err(anyhow!("injected watch processor failure")) }),
            background: shutdown_waiter(shutdown.clone()),
            index: index_worker,
        },
    ));
    drop(task_store_lease);
    drop(daemon_instance_lease);
    let outcome = tokio::time::timeout(Duration::from_millis(500), supervisor)
        .await
        .expect("supervisor exceeded its shared shutdown deadline")
        .unwrap();
    assert!(
        supervisor_started.elapsed() < Duration::from_millis(500),
        "supervisor did not detach blocking work at the shared deadline"
    );
    let error = outcome
        .result
        .expect_err("injected processor failure was not fatal");
    assert!(
        format!("{error:#}").contains("injected watch processor failure"),
        "unexpected supervisor error: {error:#}"
    );
    assert!(shutdown.is_requested());
    assert!(state.lock().unwrap().shutting_down);
    assert!(
        !ready_path(&root).exists(),
        "fatal processor exit left daemon readiness published"
    );
    assert!(
        packet28_daemon_core::task_store_lease::try_acquire_task_store_retention_lease(&root)
            .unwrap()
            .is_none(),
        "exclusive retention acquired while detached blocking owners were still running"
    );
    assert!(
        packet28_daemon_core::task_store_lease::acquire_daemon_instance_lease(&root).is_err(),
        "a second daemon acquired the instance lease while detached owners were still running"
    );

    release_tx.send(()).unwrap();
    index_release_tx.send(()).unwrap();
    blocking_worker.await.unwrap().unwrap();
    assert!(
        load_task_registry(&root)
            .unwrap()
            .tasks
            .contains_key("task-during-shutdown"),
        "detached task-store mutation did not persist before releasing its lease"
    );
    let retention_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if packet28_daemon_core::task_store_lease::try_acquire_task_store_retention_lease(&root)
            .unwrap()
            .is_some()
        {
            break;
        }
        assert!(
            Instant::now() < retention_deadline,
            "exclusive retention did not become available after detached owners exited"
        );
        tokio::task::yield_now().await;
    }
    assert!(
        packet28_daemon_core::task_store_lease::acquire_daemon_instance_lease(&root).is_ok(),
        "daemon instance lease remained held after detached owners exited"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn early_index_worker_failure_withdraws_readiness_and_stops_peer_owners() {
    let state = daemon_test_state();
    let root = state.lock().unwrap().root.clone();
    std::fs::write(ready_path(&root), b"ready\n").unwrap();
    let shutdown = state.lock().unwrap().shutdown.clone();

    let outcome = tokio::time::timeout(
        Duration::from_secs(1),
        supervise_daemon_tasks(
            state.clone(),
            shutdown.clone(),
            BlockingPool::new(1),
            Duration::from_millis(250),
            DaemonRuntimeTasks {
                transport: shutdown_waiter(shutdown.clone()),
                watch: shutdown_waiter(shutdown.clone()),
                background: shutdown_waiter(shutdown),
                index: tokio::task::spawn_blocking(|| Err(anyhow!("injected early index failure"))),
            },
        ),
    )
    .await
    .expect("early index failure did not stop daemon owners");
    let error = outcome
        .result
        .expect_err("early index failure was not fatal");

    assert!(format!("{error:#}").contains("injected early index failure"));
    assert!(state.lock().unwrap().shutting_down);
    assert!(!ready_path(&root).exists());
}

#[test]
fn production_cache_finalizer_is_bounded_and_retryable_when_root_lock_is_blocked() {
    let state = daemon_test_state();
    let (root, kernel) = {
        let guard = state.lock().unwrap();
        (guard.root.clone(), guard.kernel.clone())
    };
    let lock_path = root.join(".packet28/packet-cache-v3.lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    FileExt::lock_exclusive(&lock).unwrap();
    kernel
        .execute(KernelRequest {
            target: "agenty.state.write".to_string(),
            reducer_input: json!({
                "task_id": "task-blocked-cache-shutdown",
                "event_id": "event-1",
                "occurred_at_unix": 1,
                "actor": "agent",
                "kind": "focus_set",
                "paths": ["src/cache.rs"],
                "data": {"type": "focus_set"}
            }),
            ..KernelRequest::default()
        })
        .unwrap();

    let started = Instant::now();
    let error = shutdown_persistent_kernels(&state, Duration::from_millis(25))
        .expect_err("blocked persistence root unexpectedly shut down");
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "production cache shutdown exceeded its lifecycle bound: {:?}",
        started.elapsed()
    );
    assert!(
        format!("{error:#}").contains("timed out"),
        "unexpected bounded shutdown error: {error:#}"
    );

    FileExt::unlock(&lock).unwrap();
    shutdown_persistent_kernels(&state, Duration::from_secs(2))
        .expect("cache persistence shutdown did not succeed on bounded retry");
}

#[test]
fn blocked_persistent_root_cannot_consume_later_roots_shutdown_budget() {
    let state = daemon_test_state();
    let (root, primary, registry) = {
        let guard = state.lock().unwrap();
        (
            guard.root.clone(),
            guard.kernel.clone(),
            guard.kernel_registry.clone(),
        )
    };
    let secondary_root = root.join("secondary-root");
    std::fs::create_dir_all(&secondary_root).unwrap();
    let secondary = registry.get(&secondary_root).unwrap();
    let lock_path = root.join(".packet28/packet-cache-v3.lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    FileExt::lock_exclusive(&lock).unwrap();
    primary
        .execute(KernelRequest {
            target: "agenty.state.write".to_string(),
            reducer_input: json!({
                "task_id": "task-fair-cache-shutdown",
                "event_id": "event-1",
                "occurred_at_unix": 1,
                "actor": "agent",
                "kind": "focus_set",
                "paths": ["src/fair-cache.rs"],
                "data": {"type": "focus_set"}
            }),
            ..KernelRequest::default()
        })
        .unwrap();

    let started = Instant::now();
    let error = shutdown_persistent_kernels(&state, Duration::from_millis(200))
        .expect_err("blocked primary root unexpectedly shut down");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "multi-root cache shutdown exceeded its shared deadline: {:?}",
        started.elapsed()
    );
    assert!(format!("{error:#}").contains("timed out"));
    assert!(
        secondary
            .flush_cache_persistence(Duration::from_millis(25))
            .is_err(),
        "later persistent root was not shut down after the earlier root consumed its fair share"
    );

    FileExt::unlock(&lock).unwrap();
    shutdown_persistent_kernels(&state, Duration::from_secs(2))
        .expect("multi-root cache persistence shutdown did not succeed on bounded retry");
}

#[test]
fn slow_subscriber_is_closed_at_capacity_and_can_replay_exact_gap() {
    let state = daemon_test_state();
    insert_admitted_task_record(
        &state,
        TaskRecord {
            task_id: "task-slow".to_string(),
            ..TaskRecord::default()
        },
    );
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    state.lock().unwrap().subscribers.insert(
        "task-slow".to_string(),
        vec![crate::state::TaskSubscriber { id: 7, sender }],
    );

    emit_task_event(state.clone(), "task-slow", "first", json!({"ordinal": 1})).unwrap();
    emit_task_event(state.clone(), "task-slow", "second", json!({"ordinal": 2})).unwrap();

    assert!(!state.lock().unwrap().subscribers.contains_key("task-slow"));
    assert_eq!(receiver.try_recv().unwrap().seq, 1);
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
    ));

    let root = state.lock().unwrap().root.clone();
    let replay = crate::server::select_replay_events(
        load_task_events(&root, "task-slow").unwrap(),
        0,
        Some(1),
    );
    assert_eq!(
        replay.iter().map(|frame| frame.seq).collect::<Vec<_>>(),
        vec![2]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tcp_stop_acknowledges_stops_accepting_and_joins_connection() {
    let _tcp_test_guard = TCP_TRANSPORT_TEST_LOCK.lock().await;
    let state = daemon_test_state();
    let listener = bind_tcp_listener("transport parity test").unwrap();
    let endpoint = listener.endpoint();
    let address = endpoint
        .strip_prefix("tcp://")
        .unwrap()
        .parse::<std::net::SocketAddr>()
        .unwrap();
    let (watch_tx, _watch_rx) = WatchIngress::new(8);
    let config = DaemonRuntimeConfig {
        shutdown_grace: Duration::from_secs(2),
        ..DaemonRuntimeConfig::default()
    };
    let server_state = state.clone();
    let server = tokio::spawn(run_transport(
        listener,
        server_state,
        watch_tx,
        BlockingPool::new(2),
        config,
    ));
    let mut client = tokio::net::TcpStream::connect(address).await.unwrap();

    assert!(matches!(
        exchange(&mut client, &DaemonRequest::Status).await,
        DaemonResponse::Status { .. }
    ));
    assert!(matches!(
        exchange(&mut client, &DaemonRequest::Stop).await,
        DaemonResponse::Ack { ref message } if message == "stopping"
    ));
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("TCP transport did not stop")
        .unwrap()
        .unwrap();

    drop(client);
    let reconnect = tokio::net::TcpStream::connect(address).await;
    assert!(
        matches!(
            reconnect,
            Err(ref error) if error.kind() == ErrorKind::ConnectionRefused
        ),
        "TCP endpoint still accepted connections after structured shutdown: {reconnect:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unix_transport_matches_tcp_status_and_stop_semantics() {
    let state = daemon_test_state();
    let directory = tempfile::TempDir::new().unwrap();
    let socket = directory.path().join("packet28d.sock");
    let listener = DaemonListener::Unix {
        endpoint: socket.clone(),
        listener: UnixListener::bind(&socket).unwrap(),
    };
    let (watch_tx, _watch_rx) = WatchIngress::new(8);
    let config = DaemonRuntimeConfig {
        shutdown_grace: Duration::from_secs(2),
        ..DaemonRuntimeConfig::default()
    };
    let server_state = state.clone();
    let server = tokio::spawn(run_transport(
        listener,
        server_state,
        watch_tx,
        BlockingPool::new(2),
        config,
    ));
    let mut client = tokio::net::UnixStream::connect(&socket).await.unwrap();

    assert!(matches!(
        exchange(&mut client, &DaemonRequest::Status).await,
        DaemonResponse::Status { .. }
    ));
    assert!(matches!(
        exchange(&mut client, &DaemonRequest::Stop).await,
        DaemonResponse::Ack { ref message } if message == "stopping"
    ));
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("Unix transport did not stop")
        .unwrap()
        .unwrap();

    drop(client);
    std::fs::remove_file(&socket).unwrap();
    UnixListener::bind(&socket).expect("Unix endpoint remained in use after structured shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_cap_rejects_excess_idle_peer() {
    let _tcp_test_guard = TCP_TRANSPORT_TEST_LOCK.lock().await;
    let state = daemon_test_state();
    let listener = bind_tcp_listener("connection cap test").unwrap();
    let endpoint = listener.endpoint();
    let address = endpoint
        .strip_prefix("tcp://")
        .unwrap()
        .parse::<std::net::SocketAddr>()
        .unwrap();
    let (watch_tx, _watch_rx) = WatchIngress::new(8);
    let config = DaemonRuntimeConfig {
        max_connections: 1,
        frame_header_timeout: Duration::from_secs(5),
        shutdown_grace: Duration::from_secs(2),
        ..DaemonRuntimeConfig::default()
    };
    let server_state = state.clone();
    let server = tokio::spawn(run_transport(
        listener,
        server_state,
        watch_tx,
        BlockingPool::new(1),
        config,
    ));

    let mut first = tokio::net::TcpStream::connect(address).await.unwrap();
    first.write_all(&[0_u8]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut excess = tokio::net::TcpStream::connect(address).await.unwrap();
    let mut encoded = Vec::new();
    write_frame(&mut encoded, &DaemonRequest::Status).unwrap();
    excess.write_all(&encoded).await.unwrap();
    let mut byte = [0_u8; 1];
    let read = tokio::time::timeout(Duration::from_secs(1), excess.read(&mut byte))
        .await
        .expect("excess connection was neither rejected nor serviced");
    let rejected = match &read {
        Ok(0) => true,
        Err(error) => matches!(
            error.kind(),
            ErrorKind::ConnectionReset | ErrorKind::BrokenPipe
        ),
        Ok(_) => false,
    };
    assert!(
        rejected,
        "excess connection unexpectedly received data: {read:?}"
    );

    state.lock().unwrap().shutdown.request();
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("capped transport did not join")
        .unwrap()
        .unwrap();
}
