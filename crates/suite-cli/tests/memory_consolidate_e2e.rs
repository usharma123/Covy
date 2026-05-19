use assert_cmd::Command;
use predicates::prelude::*;
use rusqlite::Connection;
use serde_json::{self, Value};
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_memory_consolidate_preserves_metadata_and_deletes_sources() {
    let home = TempDir::new().unwrap();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Consolidation source one keeps parser context",
            "--tags",
            "parser,cli",
            "--topic",
            "consolidation-meta",
            "--importance",
            "low",
            "--keywords",
            "parser,context",
            "--project",
            "coverage-a",
            "--source",
            "source-one",
            "--raw",
            "raw parser context",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "store",
            "Consolidation source two keeps daemon context",
            "--tags",
            "daemon,cli",
            "--topic",
            "consolidation-meta",
            "--importance",
            "critical",
            "--keywords",
            "daemon,context",
            "--project",
            "coverage-b",
            "--source",
            "source-two",
            "--raw",
            "raw daemon context",
            "--json",
        ])
        .assert()
        .success();

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "extract-patterns",
            "--topic",
            "consolidation-meta",
            "--memoir",
            "ConsolidationPatterns",
            "--min-cluster-size",
            "2",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"pattern_count\""))
        .stdout(predicate::str::contains("\"key\":\"context\""))
        .stdout(predicate::str::contains("\"created_concepts\""));

    let output = suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "consolidate",
            "--topic",
            "consolidation-meta",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "consolidate failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "consolidated");
    assert_eq!(report["source_count"], 2);
    assert_eq!(report["consolidated_memory"]["importance"], "critical");
    assert_eq!(report["consolidated_memory"]["tags"], "daemon,cli,parser");
    assert_eq!(
        report["consolidated_memory"]["keywords"],
        "daemon,context,parser"
    );
    assert_eq!(
        report["consolidated_memory"]["project"],
        "coverage-b,coverage-a"
    );
    assert_eq!(
        report["consolidated_memory"]["source"],
        "source-two,source-one"
    );
    let raw_excerpt = report["consolidated_memory"]["raw_excerpt"]
        .as_str()
        .unwrap();
    assert!(raw_excerpt.contains("raw daemon context"));
    assert!(raw_excerpt.contains("raw parser context"));

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "memory",
            "list",
            "--topic",
            "consolidation-meta",
            "--all",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"content\":\"Consolidated memory",
        ))
        .stdout(predicate::str::contains("\"importance\":\"critical\""))
        .stdout(predicate::str::contains("\"memory_count\"").not());

    let conn = Connection::open(home.path().join(".packet28").join("packet28.db")).unwrap();
    let memory_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE topic = 'consolidation-meta'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let chunk_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM memory_chunks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(memory_count, 1);
    assert_eq!(chunk_count, 1);
}
