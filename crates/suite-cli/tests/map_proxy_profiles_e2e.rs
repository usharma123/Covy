#[path = "support/map_proxy.rs"]
mod map_proxy;
#[path = "support/map_proxy_packet.rs"]
mod map_proxy_packet;
#[path = "support/map_proxy_payload.rs"]
mod map_proxy_payload;
#[path = "support/map_proxy_repo.rs"]
mod map_proxy_repo;

use map_proxy::suite_cmd;
use map_proxy_packet::parse_packet_wrapper;
use map_proxy_payload::packet_payload;
use map_proxy_repo::write_repo_fixture;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

#[test]
fn test_map_proxy_profiles_map_repo_and_handle_fetch_share_hash() {
    let dir = TempDir::new().unwrap();
    write_repo_fixture(dir.path());

    let compact_output = suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--json=compact",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let compact = parse_packet_wrapper(&compact_output, "suite.map.repo.v1");

    let full_output = suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--json=full",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let full = parse_packet_wrapper(&full_output, "suite.map.repo.v1");

    let handle_output = suite_cmd()
        .args([
            "map",
            "repo",
            "--repo-root",
            dir.path().to_str().unwrap(),
            "--json=handle",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let handle = parse_packet_wrapper(&handle_output, "suite.map.repo.v1");

    let compact_hash = compact
        .get("packet")
        .and_then(|packet| packet.get("hash"))
        .and_then(Value::as_str)
        .unwrap();
    assert!(packet_payload(&compact)
        .get("files_ranked")
        .and_then(Value::as_array)
        .and_then(|files| files.first())
        .and_then(|file| file.get("path"))
        .and_then(Value::as_str)
        .is_some());
    assert!(packet_payload(&compact)
        .get("symbols_ranked")
        .and_then(Value::as_array)
        .and_then(|symbols| symbols.first())
        .and_then(|symbol| symbol.get("name"))
        .and_then(Value::as_str)
        .is_some());

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
    assert!(packet_payload(&handle)
        .get("files_ranked")
        .and_then(Value::as_array)
        .and_then(|files| files.first())
        .and_then(|file| file.get("path"))
        .and_then(Value::as_str)
        .is_some());
    let handle_id = artifact_handle
        .get("handle_id")
        .and_then(Value::as_str)
        .unwrap();
    let artifact_path = artifact_handle.get("path").and_then(Value::as_str).unwrap();
    assert!(Path::new(artifact_path).exists());

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
    let fetched = parse_packet_wrapper(&fetch_output, "suite.map.repo.v1");
    let fetched_hash = fetched
        .get("packet")
        .and_then(|packet| packet.get("hash"))
        .and_then(Value::as_str)
        .unwrap();
    assert_eq!(compact_hash, fetched_hash);
}
