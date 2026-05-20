#[path = "support/impact.rs"]
mod impact;

use impact::{build_basic_testmap, covy_cmd};
use predicates::prelude::*;
use tempfile::TempDir;

#[test]
fn test_impact_print_command_outputs_helper() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let testmap = dir.path().join("testmap.bin");
    build_basic_testmap(&manifest, &testmap);

    covy_cmd()
        .args([
            "impact",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--testmap",
            testmap.to_str().unwrap(),
            "--print-command",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("echo \"no impacted tests\""));
}

#[test]
fn test_impact_legacy_mode_still_works_without_warning_noise() {
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("manifest.jsonl");
    let testmap = dir.path().join("testmap.bin");
    build_basic_testmap(&manifest, &testmap);

    covy_cmd()
        .args([
            "impact",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "--testmap",
            testmap.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("deprecated").not());
}
