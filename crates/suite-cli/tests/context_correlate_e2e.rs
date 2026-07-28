#[path = "support/context_correlate.rs"]
mod context_correlate;
#[path = "support/packet_wrapper.rs"]
mod packet_wrapper;

use context_correlate::{suite_cmd, write_correlation_packets};
use packet_wrapper::{packet_payload, parse_packet_wrapper};
use serde_json::Value;
use tempfile::TempDir;

#[test]
fn test_context_correlate_cli_emits_v1_findings() {
    let dir = TempDir::new().unwrap();
    let packets = write_correlation_packets(dir.path());

    let output = suite_cmd()
        .args([
            "context",
            "correlate",
            "--packet",
            packets.diff.to_str().unwrap(),
            "--packet",
            packets.impact.to_str().unwrap(),
            "--packet",
            packets.stack.to_str().unwrap(),
            "--packet",
            packets.build.to_str().unwrap(),
            "--packet",
            packets.map.to_str().unwrap(),
            "--task-id",
            "task-correlation",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value = parse_packet_wrapper(&output, "suite.context.correlate.v1");
    let findings = packet_payload(&value)
        .get("findings")
        .and_then(Value::as_array)
        .unwrap();
    assert!(findings.len() >= 3);
    assert!(findings
        .iter()
        .any(|finding| { finding.get("relation").and_then(Value::as_str) == Some("unrelated") }));
    assert!(findings
        .iter()
        .any(|finding| { finding.get("relation").and_then(Value::as_str) == Some("supports") }));
    assert!(findings.iter().any(|finding| {
        finding.get("relation").and_then(Value::as_str) == Some("pre_existing_or_unrelated")
    }));
    assert!(findings
        .iter()
        .any(|finding| { finding.get("rule").and_then(Value::as_str) == Some("shared_file") }));
}
