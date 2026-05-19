use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{self, Value};
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_memory_recall_scores_importance_and_keywords() {
    let home = TempDir::new().unwrap();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Packet28 ranking shared term high signal",
            "--topic",
            "scoring",
            "--importance",
            "high",
            "--keywords",
            "priority,ranking",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"weight\":0.9"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Packet28 ranking shared term low signal",
            "--topic",
            "scoring",
            "--importance",
            "low",
            "--keywords",
            "archive",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"weight\":0.5"));

    let output = suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "recall",
            "ranking shared term",
            "--topic",
            "scoring",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "recall failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records: Value = serde_json::from_slice(&output.stdout).unwrap();
    let records = records.as_array().unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["importance"], "high");
    assert_eq!(records[1]["importance"], "low");
    let high_score = records[0]["recall_score"].as_f64().unwrap();
    let low_score = records[1]["recall_score"].as_f64().unwrap();
    assert!(
        high_score > low_score,
        "high importance keyword score {high_score} should exceed low score {low_score}"
    );

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Packet28 fts calibration keeps the exact phrase together",
            "--topic",
            "fts-calibration",
            "--importance",
            "medium",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Packet28 fts calibration mentions exact and later mentions phrase",
            "--topic",
            "fts-calibration",
            "--importance",
            "medium",
            "--json",
        ])
        .assert()
        .success();
    let output = suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "recall",
            "exact phrase",
            "--topic",
            "fts-calibration",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fts recall failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records: Value = serde_json::from_slice(&output.stdout).unwrap();
    let records = records.as_array().unwrap();
    assert_eq!(records.len(), 2);
    assert!(records[0]["content"]
        .as_str()
        .unwrap()
        .contains("exact phrase together"));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "update",
            "2",
            "--importance",
            "critical",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"importance\":\"critical\""))
        .stdout(predicate::str::contains("\"weight\":1.0"));
}
