#[path = "support/map_proxy.rs"]
mod map_proxy;
#[path = "support/map_proxy_packet.rs"]
mod map_proxy_packet;
#[path = "support/map_proxy_payload.rs"]
mod map_proxy_payload;

use map_proxy::suite_cmd;
use map_proxy_packet::parse_packet_wrapper;
use map_proxy_payload::packet_payload;
use serde_json::Value;
use tempfile::TempDir;

#[test]
fn test_map_proxy_profiles_proxy_run_and_handle_fetch_share_hash() {
    let dir = TempDir::new().unwrap();

    let compact_output = suite_cmd()
        .args([
            "proxy",
            "run",
            "--cwd",
            dir.path().to_str().unwrap(),
            "--json=compact",
            "--",
            "ls",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let compact = parse_packet_wrapper(&compact_output, "suite.proxy.run.v1");

    let full_output = suite_cmd()
        .args([
            "proxy",
            "run",
            "--cwd",
            dir.path().to_str().unwrap(),
            "--json=full",
            "--",
            "ls",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let full = parse_packet_wrapper(&full_output, "suite.proxy.run.v1");

    let handle_output = suite_cmd()
        .args([
            "proxy",
            "run",
            "--cwd",
            dir.path().to_str().unwrap(),
            "--json=handle",
            "--",
            "ls",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let handle = parse_packet_wrapper(&handle_output, "suite.proxy.run.v1");

    let compact_hash = compact
        .get("packet")
        .and_then(|packet| packet.get("hash"))
        .and_then(Value::as_str)
        .unwrap();
    let full_hash = full
        .get("packet")
        .and_then(|packet| packet.get("hash"))
        .and_then(Value::as_str)
        .unwrap();
    let handle_hash = handle
        .get("packet")
        .and_then(|packet| packet.get("hash"))
        .and_then(Value::as_str)
        .unwrap();
    assert_eq!(compact_hash, full_hash);
    assert_eq!(compact_hash, handle_hash);

    let artifact_handle = packet_payload(&handle)
        .get("artifact_handle")
        .cloned()
        .unwrap();
    let handle_id = artifact_handle
        .get("handle_id")
        .and_then(Value::as_str)
        .unwrap();

    let fetch_output = suite_cmd()
        .args([
            "packet",
            "fetch",
            "--handle",
            handle_id,
            "--root",
            dir.path().to_str().unwrap(),
            "--json=full",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let fetched = parse_packet_wrapper(&fetch_output, "suite.proxy.run.v1");
    let fetched_hash = fetched
        .get("packet")
        .and_then(|packet| packet.get("hash"))
        .and_then(Value::as_str)
        .unwrap();
    assert_eq!(compact_hash, fetched_hash);
}
