#[path = "support/feedback_graph.rs"]
mod feedback_graph;

use feedback_graph::suite_cmd;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_feedback_graph_cli_learn_and_graph_use_sqlite() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nname = \"cli-learn-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nserde_json = \"1\"\n",
    )
    .unwrap();
    fs::create_dir_all(project.path().join("src")).unwrap();
    fs::write(project.path().join("src/main.rs"), "fn main() {}\n").unwrap();

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "learn",
            "--project-dir",
            project.path().to_str().unwrap(),
            "--project-name",
            "CliLearnFixture",
            "--memoir",
            "CliLearnMemoir",
            "--project-limit",
            "5",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"project_name\":\"CliLearnFixture\"",
        ))
        .stdout(predicate::str::contains(
            "\"memoir_name\":\"CliLearnMemoir\"",
        ))
        .stdout(predicate::str::contains("\"link_count\""))
        .stdout(predicate::str::contains("serde_json"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "show", "CliLearnMemoir", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("CliLearnFixture"))
        .stdout(predicate::str::contains("serde_json"));

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
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "delete", "Packet28", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"deleted_concepts\":1"));
}
