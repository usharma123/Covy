use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn daemon_test_state() -> Arc<Mutex<DaemonState>> {
    daemon_test_state_with_persistence_debounce(Duration::from_millis(TASK_PERSISTENCE_DEBOUNCE_MS))
}

pub(crate) fn daemon_test_state_with_persistence_debounce(
    persistence_debounce: Duration,
) -> Arc<Mutex<DaemonState>> {
    let root = std::env::temp_dir().join(format!(
        "packet28-broker-test-{}-{}-{}",
        now_unix_millis(),
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    ensure_daemon_dir(&root).unwrap();
    let kernel = Arc::new(Kernel::with_v1_reducers_and_persistence(
        PersistConfig::new(root.clone()),
    ));
    let kernel_registry = Arc::new(
        crate::kernel_registry::PersistentKernelRegistry::new(&root, kernel.clone(), 4).unwrap(),
    );
    let (index_tx, index_rx) = IndexIngress::new();
    thread::spawn(move || index_rx.discard_until_shutdown());
    let (background_tx, mut background_rx) = tokio::sync::mpsc::channel(8);
    thread::spawn(move || while background_rx.blocking_recv().is_some() {});
    let (persistence_owner, persistence) =
        PersistenceOwner::start_unleased(root.clone(), persistence_debounce).unwrap();
    Arc::new(Mutex::new(DaemonState {
        root,
        kernel,
        kernel_registry,
        runtime: DaemonRuntimeInfo::default(),
        tasks: TaskRegistry::default(),
        task_generations: TaskGenerationRegistry::default(),
        agent_snapshots: BTreeMap::new(),
        watches: WatchRegistry::default(),
        registry_instance_id: "test-registry-instance".to_string(),
        registry_page_index: None,
        watcher_handles: HashMap::new(),
        subscribers: HashMap::new(),
        source_file_cache: BTreeMap::new(),
        interactive_index: InteractiveIndexRuntime::default(),
        index_tx,
        background_tx,
        persistence,
        _persistence_owner: Some(persistence_owner),
        shutdown: ShutdownSignal::new(),
        changes: StateChangeSignal::new(),
        shutting_down: false,
    }))
}

pub(crate) fn daemon_test_root(state: &Arc<Mutex<DaemonState>>) -> PathBuf {
    state.lock().unwrap().root.clone()
}

pub(crate) fn refresh_test_repo_runtime(state: &Arc<Mutex<DaemonState>>) {
    let root = daemon_test_root(state);
    let repo_runtime =
        mapy_core::rebuild_repo_index_runtime(&root, true).expect("build persisted map runtime");
    let mut guard = state.lock().expect("map state");
    guard.interactive_index.repo_runtime = Some(repo_runtime);
    guard.interactive_index.manifest.status = DaemonIndexState::Ready;
    guard.interactive_index.manifest.dirty_paths.clear();
    guard.interactive_index.manifest.queued_paths.clear();
}

pub(crate) fn insert_admitted_task_record(state: &Arc<Mutex<DaemonState>>, record: TaskRecord) {
    let mut guard = state.lock().unwrap();
    guard.tasks.tasks.insert(record.task_id.clone(), record);
    persist_state_for_test(&guard).unwrap();
}

pub(crate) fn shutdown_test_persistence(state: &Arc<Mutex<DaemonState>>) {
    let owner = state
        .lock()
        .unwrap()
        ._persistence_owner
        .take()
        .expect("test persistence owner is present");
    owner.shutdown(Duration::from_secs(5)).unwrap();
}

pub(super) fn broker_evidence_confidence_body(
    state: &Arc<Mutex<DaemonState>>,
    snapshot: suite_packet_core::AgentSnapshotPayload,
) -> String {
    let root = daemon_test_root(state);
    build_broker_sections(
        &root,
        state,
        &BrokerGetContextRequest {
            task_id: "task-confidence".to_string(),
            action: Some(BrokerAction::Inspect),
            include_sections: vec!["evidence_confidence".to_string()],
            ..BrokerGetContextRequest::default()
        },
        &snapshot,
        None,
        None,
    )
    .into_iter()
    .find(|section| section.id == "evidence_confidence")
    .expect("evidence confidence should render")
    .body
}

pub(super) fn broker_evidence_confidence_reason_line(body: &str) -> &str {
    body.lines()
        .find(|line| line.starts_with("- confidence_reason:"))
        .expect("confidence reason line should render")
}

pub(super) fn broker_context_debt_body(
    state: &Arc<Mutex<DaemonState>>,
    snapshot: suite_packet_core::AgentSnapshotPayload,
) -> Option<String> {
    let root = daemon_test_root(state);
    build_broker_sections(
        &root,
        state,
        &BrokerGetContextRequest {
            task_id: "task-context-debt".to_string(),
            action: Some(BrokerAction::Edit),
            include_sections: vec!["context_debt".to_string()],
            ..BrokerGetContextRequest::default()
        },
        &snapshot,
        None,
        None,
    )
    .into_iter()
    .find(|section| section.id == "context_debt")
    .map(|section| section.body)
}

pub(super) fn tool_invocation(
    invocation_id: &str,
    sequence: u64,
    tool_name: &str,
    operation_kind: suite_packet_core::ToolOperationKind,
    result_summary: Option<&str>,
    artifact_id: Option<&str>,
) -> suite_packet_core::ToolInvocationSummary {
    suite_packet_core::ToolInvocationSummary {
        invocation_id: invocation_id.to_string(),
        sequence,
        tool_name: tool_name.to_string(),
        operation_kind,
        result_summary: result_summary.map(ToOwned::to_owned),
        artifact_id: artifact_id.map(ToOwned::to_owned),
        ..suite_packet_core::ToolInvocationSummary::default()
    }
}

pub(super) fn write_test_coverage_state(root: &Path, path: &str, covered: bool) {
    let mut coverage = suite_packet_core::CoverageData::new();
    let mut file = suite_packet_core::FileCoverage::new();
    file.lines_instrumented.insert(1);
    if covered {
        file.lines_covered.insert(1);
    }
    coverage.files.insert(path.to_string(), file);
    let bytes = suite_foundation_core::cache::serialize_coverage(&coverage).unwrap();
    let state_dir = root.join(".covy").join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(state_dir.join("latest.bin"), bytes).unwrap();
}

pub(super) fn write_testmap_state(root: &Path, path: &str, tests: &[&str]) {
    write_testmap_state_with_generated_at(root, path, tests, current_test_unix_seconds());
}

pub(super) fn write_testmap_state_with_generated_at(
    root: &Path,
    path: &str,
    tests: &[&str],
    generated_at: u64,
) {
    let mut index = suite_packet_core::TestMapIndex::default();
    index.metadata.generated_at = generated_at;
    index.file_to_tests.insert(
        path.to_string(),
        tests.iter().map(|test| (*test).to_string()).collect(),
    );
    let state_dir = root.join(".covy").join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    testy_core::pipeline_testmap::write_testmap(&state_dir.join("testmap.bin"), &index).unwrap();
}

fn current_test_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(super) fn write_search_fixture(root: &Path, files: &[(&str, &str)]) {
    for (relative_path, contents) in files {
        let path = root.join(relative_path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
}

pub(super) fn run_search_execution_for_query(
    root: &Path,
    query: &str,
    action: BrokerAction,
) -> SearchExecution {
    let snapshot = suite_packet_core::AgentSnapshotPayload::default();
    run_search_execution_for_query_with_snapshot(root, query, action, &snapshot)
}

pub(super) fn run_search_execution_for_query_with_snapshot(
    root: &Path,
    query: &str,
    action: BrokerAction,
    snapshot: &suite_packet_core::AgentSnapshotPayload,
) -> SearchExecution {
    let regex_runtime = packet28_search_core::rebuild_full_index(root, true)
        .expect("build persisted search runtime");
    let state = daemon_test_state();
    {
        let mut guard = state.lock().expect("search state");
        guard.root = root.to_path_buf();
        guard.interactive_index.regex_runtime = Some(regex_runtime);
        guard.interactive_index.manifest.status = DaemonIndexState::Ready;
    }
    let request = BrokerGetContextRequest {
        task_id: "task-search".to_string(),
        action: Some(action),
        query: Some(query.to_string()),
        ..BrokerGetContextRequest::default()
    };
    let query_focus = derive_query_focus(Some(query));
    build_reducer_search_execution(SearchExecutionArgs {
        state: Some(&state),
        root,
        snapshot,
        request: &request,
        query_focus: &query_focus,
        action,
        max_files: 8,
        max_evidence_lines: 8,
    })
}
