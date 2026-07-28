use super::support::daemon_test_state;
use super::*;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

static TCP_TRANSPORT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

#[test]
fn slow_subscriber_is_closed_at_capacity_and_can_replay_exact_gap() {
    let state = daemon_test_state();
    {
        let mut guard = state.lock().unwrap();
        guard.tasks.tasks.insert(
            "task-slow".to_string(),
            TaskRecord {
                task_id: "task-slow".to_string(),
                ..TaskRecord::default()
            },
        );
    }
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
