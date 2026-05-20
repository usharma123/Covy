#[path = "support/feedback_graph.rs"]
mod feedback_graph;

use feedback_graph::suite_cmd;
use predicates::prelude::*;
use tempfile::TempDir;

#[test]
fn test_feedback_graph_cli_crud_and_query_use_sqlite() {
    let home = TempDir::new().unwrap();

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "graph",
            "create",
            "--name",
            "Packet28Memoir",
            "--description",
            "Packet28 graph parity evidence",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Packet28Memoir"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "graph",
            "add-concept",
            "Packet28",
            "--memoir",
            "Packet28Memoir",
            "--label",
            "domain:context",
            "--confidence",
            "0.82",
            "--source-id",
            "memory:packet28",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Packet28"))
        .stdout(predicate::str::contains(
            "\"memoir_name\":\"Packet28Memoir\"",
        ))
        .stdout(predicate::str::contains("domain:context"))
        .stdout(predicate::str::contains("\"confidence\":0.82"))
        .stdout(predicate::str::contains("memory:packet28"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "graph",
            "refine",
            "Packet28",
            "local context runtime with reducers",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "local context runtime with reducers",
        ));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "graph",
            "add-concept",
            "Reducers",
            "--memoir",
            "Packet28Memoir",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "graph",
            "link",
            "Packet28",
            "Reducers",
            "--relation",
            "uses",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "graph",
            "search",
            "context",
            "--memoir",
            "Packet28Memoir",
            "--label",
            "domain:context",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Packet28"))
        .stdout(predicate::str::contains("domain:context"))
        .stdout(predicate::str::contains("Packet28Memoir"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "export", "--format", "dot"])
        .assert()
        .success()
        .stdout(predicate::str::contains("digraph packet28_graph"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"relation\":\"uses\""));
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "list", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Packet28Memoir"))
        .stdout(predicate::str::contains("\"concept_count\":2"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "show", "Packet28Memoir", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"revision\":2"))
        .stdout(predicate::str::contains("\"average_confidence\":0.659"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "inspect", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Packet28"))
        .stdout(predicate::str::contains("Reducers"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "graph",
            "inspect-concept",
            "Packet28",
            "--memoir",
            "Packet28Memoir",
            "--depth",
            "1",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"concept\""))
        .stdout(predicate::str::contains("\"neighbors\""))
        .stdout(predicate::str::contains("\"relations\""))
        .stdout(predicate::str::contains("Reducers"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "delete", "Packet28", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"deleted_concepts\":1"));
}
