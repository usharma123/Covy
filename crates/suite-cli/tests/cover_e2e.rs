use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::path::PathBuf;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn fixture(rel: &str) -> String {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    workspace
        .join("tests")
        .join("fixtures")
        .join(rel)
        .to_string_lossy()
        .to_string()
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
fn test_cover_cli_check_smoke() {
    let output = suite_cmd()
        .args([
            "cover",
            "check",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value = parse_packet_wrapper(&output, "suite.cover.check.v1");
    assert!(packet_payload(&value).get("passed").is_some());
}

#[test]
fn test_cover_cli_check_rich_json_smoke() {
    let output = suite_cmd()
        .args([
            "cover",
            "check",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--json",
            "--packet-detail",
            "rich",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value = parse_packet_wrapper(&output, "suite.cover.check.v1");
    assert!(packet_payload(&value).get("violations").is_some());
}

#[test]
fn test_cover_cli_check_terminal_default() {
    suite_cmd()
        .args([
            "cover",
            "check",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Quality Gate: PASSED"))
        .stdout(predicate::str::contains("\"schema_version\"").not());
}

#[test]
fn test_cover_cli_check_report_json_compat_maps_to_packet_wrapper() {
    let output = suite_cmd()
        .args([
            "cover",
            "check",
            "--coverage",
            &fixture("lcov/basic.info"),
            "--no-issues-state",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--report",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value = parse_packet_wrapper(&output, "suite.cover.check.v1");
    assert!(packet_payload(&value).get("passed").is_some());
}
