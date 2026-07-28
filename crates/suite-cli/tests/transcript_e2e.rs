use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_transcript_round_trip_export_import() {
    let home_a = TempDir::new().unwrap();
    let home_b = TempDir::new().unwrap();
    let export_path = home_a.path().join("transcripts.json");

    suite_cmd()
        .env("HOME", home_a.path())
        .args([
            "transcript",
            "append",
            "Exported transcript context",
            "--session",
            "export-session",
            "--agent",
            "codex",
            "--role",
            "assistant",
            "--source",
            "fixture",
        ])
        .assert()
        .success();

    suite_cmd()
        .env("HOME", home_a.path())
        .args([
            "transcript",
            "export",
            "--session",
            "export-session",
            "--output",
            export_path.to_str().unwrap(),
            "--pretty",
        ])
        .assert()
        .success();
    assert!(fs::read_to_string(&export_path)
        .unwrap()
        .contains("packet28.transcript.export"));

    suite_cmd()
        .env("HOME", home_b.path())
        .args([
            "transcript",
            "import",
            export_path.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"imported_count\":1"));

    suite_cmd()
        .env("HOME", home_b.path())
        .args(["transcript", "show", "export-session", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Exported transcript context"))
        .stdout(predicate::str::contains("\"agent\":\"codex\""));
}
