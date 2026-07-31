use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

pub fn corrupt_task_event_log(root: &Path, task_id: &str) {
    use packet28_daemon_protocol::paths::{task_event_log_path, TaskStorageId};

    let storage_id = TaskStorageId::try_from(task_id).unwrap();
    let event_path = task_event_log_path(root, &storage_id);
    fs::create_dir_all(event_path.parent().unwrap()).unwrap();
    super::process_harness::replace_file_with_directory(&event_path);
    assert!(
        packet28_daemon_core::storage::load_task_events_from_offset(root, task_id, 0).is_err(),
        "unreadable event-log fixture remained readable"
    );
}

pub fn wait_for_file(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for fixture marker '{}'",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn large_response_batch() -> Value {
    let id_suffix = "x".repeat(32 * 1024);
    Value::Array(
        (0..128)
            .map(|index| {
                json!({
                    "jsonrpc":"2.0",
                    "id":format!("{index}-{id_suffix}"),
                    "method":"prompts/list"
                })
            })
            .collect(),
    )
}
