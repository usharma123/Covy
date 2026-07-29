use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::path::Path;

pub fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

pub fn ensure_packet28d_built() {
    crate::process_harness::ensure_packet28d_built();
}

fn git(root: &Path, args: &[&str]) {
    crate::process_harness::run_git(root, args);
}

pub fn init_repo(root: &Path) {
    git(root, &["init"]);
}

pub fn write_context_packet(path: &Path, packet_id: &str, title: &str, body: &str, path_ref: &str) {
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

pub fn write_packet_value(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
}
