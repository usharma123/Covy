use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn write_repo_fixture(root: &Path) {
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("alpha.rs"),
        r#"
use crate::beta::Beta;

fn alpha() {}
struct Alpha;
"#,
    )
    .unwrap();
    fs::write(
        src.join("beta.rs"),
        r#"
fn beta() {}
enum Beta {
  A,
}
"#,
    )
    .unwrap();
}

fn write_context_packet(path: &Path, packet_id: &str, title: &str, body: &str, path_ref: &str) {
    fs::write(
        path,
        format!(
            r#"{{
  "packet_id": "{packet_id}",
  "tool": "{packet_id}",
  "reducer": "reduce",
  "paths": ["{path_ref}"],
  "sections": [
    {{
      "title": "{title}",
      "body": "{body}",
      "refs": [{{ "kind": "file", "value": "{path_ref}" }}],
      "relevance": 0.9
    }}
  ]
}}"#
        ),
    )
    .unwrap();
}

fn write_state_event(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
}

fn parse_packet_wrapper(output: &[u8], packet_type: &str) -> Value {
    let value: Value = serde_json::from_slice(output).unwrap();
    assert_eq!(
        value.get("schema_version").and_then(Value::as_str),
        Some("suite.packet.v1")
    );
    assert_eq!(
        value.get("packet_type").and_then(Value::as_str),
        Some(packet_type)
    );
    assert!(value.get("packet").is_some());
    value
}

fn packet_payload(wrapper: &Value) -> &Value {
    wrapper
        .get("packet")
        .and_then(|packet| packet.get("payload"))
        .expect("packet.payload should exist")
}

#[test]
fn test_context_store_state_cli_store_list_get_prune_stats_json() {
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());

    suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--cache",
            "--json",
        ])
        .assert()
        .success();

    let stats_output = suite_cmd()
        .args([
            "context",
            "store",
            "stats",
            "--root",
            dir.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stats: Value = serde_json::from_slice(&stats_output).unwrap();
    assert_eq!(
        stats.get("schema_version").and_then(Value::as_str),
        Some("suite.context.store.stats.v1")
    );
    assert!(
        stats
            .get("stats")
            .and_then(|v| v.get("entries"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
            >= 1
    );

    let list_output = suite_cmd()
        .args([
            "context",
            "store",
            "ls",
            "--root",
            dir.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let list: Value = serde_json::from_slice(&list_output).unwrap();
    assert_eq!(
        list.get("schema_version").and_then(Value::as_str),
        Some("suite.context.store.list.v1")
    );
    let entries = list.get("entries").and_then(Value::as_array).unwrap();
    assert!(!entries.is_empty());
    let key = entries
        .first()
        .and_then(|v| v.get("cache_key"))
        .and_then(Value::as_str)
        .unwrap()
        .to_string();

    let get_output = suite_cmd()
        .args([
            "context",
            "store",
            "get",
            "--root",
            dir.path().to_str().unwrap(),
            "--key",
            key.as_str(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let get_value: Value = serde_json::from_slice(&get_output).unwrap();
    assert_eq!(
        get_value
            .get("entry")
            .and_then(|v| v.get("entry"))
            .and_then(|v| v.get("cache_key"))
            .and_then(Value::as_str),
        Some(key.as_str())
    );

    let prune_output = suite_cmd()
        .args([
            "context",
            "store",
            "gc",
            "--root",
            dir.path().to_str().unwrap(),
            "--all",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let prune_value: Value = serde_json::from_slice(&prune_output).unwrap();
    assert_eq!(
        prune_value.get("schema_version").and_then(Value::as_str),
        Some("suite.context.store.prune.v1")
    );
    assert!(
        prune_value
            .get("report")
            .and_then(|v| v.get("removed"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
            >= 1
    );
    assert!(
        prune_value
            .get("report")
            .and_then(|v| v.get("reasons"))
            .and_then(|v| v.get("manual_prune"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
            >= 1
    );
}

#[test]
fn test_context_store_state_cli_recall_returns_recent_hits() {
    let dir = TempDir::new().unwrap();
    let packet = dir.path().join("packet.json");
    write_context_packet(
        &packet,
        "diffy",
        "Parser note",
        "missing mappings in parser for src/lib.rs",
        "src/lib.rs",
    );

    suite_cmd()
        .current_dir(dir.path())
        .args([
            "context",
            "assemble",
            "--packet",
            packet.to_str().unwrap(),
            "--cache",
        ])
        .assert()
        .success();

    let output = suite_cmd()
        .args([
            "context",
            "recall",
            "--root",
            dir.path().to_str().unwrap(),
            "--query",
            "mappings parser src/lib.rs",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        value.get("schema_version").and_then(Value::as_str),
        Some("suite.context.recall.v1")
    );
    assert!(value
        .get("hits")
        .and_then(Value::as_array)
        .map(|hits| !hits.is_empty())
        .unwrap_or(false));
}

#[test]
fn test_context_store_state_cli_state_append_then_snapshot() {
    let dir = TempDir::new().unwrap();
    let event_path = dir.path().join("event.json");
    write_state_event(
        &event_path,
        r#"{
  "event_id": "evt-1",
  "occurred_at_unix": 1700000000,
  "actor": "agent",
  "kind": "question_opened",
  "data": {
    "type": "question_opened",
    "question_id": "q1",
    "text": "Does DateUtils call split()?"
  }
}"#,
    );

    let append_output = suite_cmd()
        .args([
            "context",
            "state",
            "append",
            "--task-id",
            "task-demo",
            "--input",
            event_path.to_str().unwrap(),
            "--root",
            dir.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let append = parse_packet_wrapper(&append_output, "suite.agent.state.v1");
    assert_eq!(
        packet_payload(&append)
            .get("task_id")
            .and_then(Value::as_str),
        Some("task-demo")
    );

    let snapshot_output = suite_cmd()
        .args([
            "context",
            "state",
            "snapshot",
            "--task-id",
            "task-demo",
            "--root",
            dir.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let snapshot = parse_packet_wrapper(&snapshot_output, "suite.agent.snapshot.v1");
    assert_eq!(
        packet_payload(&snapshot)
            .get("event_count")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        packet_payload(&snapshot)
            .get("open_questions")
            .and_then(Value::as_array)
            .map(|questions| questions.len()),
        Some(1)
    );
}

#[test]
fn test_context_store_state_cli_assemble_task_id_compresses_read_section() {
    let dir = TempDir::new().unwrap();
    let event_path = dir.path().join("event.json");
    let packet_path = dir.path().join("packet.json");
    write_state_event(
        &event_path,
        r#"{
  "event_id": "evt-1",
  "occurred_at_unix": 1700000000,
  "actor": "agent",
  "kind": "file_read",
  "paths": ["src/time/StopWatch.java"],
  "data": {
    "type": "file_read"
  }
}"#,
    );
    fs::write(
        &packet_path,
        r#"{
  "packet_id": "diffy",
  "sections": [
    {
      "title": "Diff",
      "body": "StopWatch.java changed on lines 10-20",
      "refs": [{"kind": "file", "value": "src/time/StopWatch.java"}],
      "relevance": 0.9
    }
  ]
}"#,
    )
    .unwrap();

    suite_cmd()
        .args([
            "context",
            "state",
            "append",
            "--task-id",
            "task-demo",
            "--input",
            event_path.to_str().unwrap(),
            "--root",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "context",
            "assemble",
            "--packet",
            packet_path.to_str().unwrap(),
            "--task-id",
            "task-demo",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.context.assemble.v1");
    let first_body = packet_payload(&value)
        .get("sections")
        .and_then(Value::as_array)
        .and_then(|sections| sections.first())
        .and_then(|section| section.get("body"))
        .and_then(Value::as_str)
        .unwrap();
    assert!(first_body.starts_with("Reminder: already reviewed"));
}
