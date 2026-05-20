#[path = "support/feedback_graph.rs"]
mod feedback_graph;

use feedback_graph::suite_cmd;
use predicates::prelude::*;
use tempfile::TempDir;

#[test]
fn test_feedback_graph_distill_creates_memory_concepts() {
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
            "memory",
            "store",
            "Reducer distillation should become a graph concept",
            "--topic",
            "graph-distill",
            "--keywords",
            "ReducerDistill,graph",
            "--json",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "graph",
            "distill",
            "--from-topic",
            "graph-distill",
            "--into",
            "Packet28Memoir",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"created_count\":2"))
        .stdout(predicate::str::contains("ReducerDistill"))
        .stdout(predicate::str::contains("\"graph\""))
        .stdout(predicate::str::contains("topic:graph-distill"))
        .stdout(predicate::str::contains("memory:"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "show", "Packet28Memoir", "--limit", "20", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ReducerDistill"))
        .stdout(predicate::str::contains("\"target\":\"graph\""))
        .stdout(predicate::str::contains("\"relation\":\"mentions\""));
}
