#[path = "support/feedback_graph.rs"]
mod feedback_graph;

use feedback_graph::suite_cmd;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_feedback_graph_learn_populates_graph() {
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
}
