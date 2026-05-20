mod support;

use std::fs;

use predicates::prelude::*;
use support::{cli, output, stderr_text, stdout_text, write_fixture};

#[test]
fn debug_build_prints_generation_and_file_count() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());

    cli()
        .args(["debug", "build", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("build_ms="))
        .stdout(predicate::str::contains("generation="))
        .stdout(predicate::str::contains("files="));
}

#[test]
fn p28_searches_from_repo_root_with_rg_style_output() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());

    cli()
        .current_dir(dir.path())
        .args(["Alpha", "--fixed-strings"])
        .assert()
        .success()
        .stdout(predicate::str::contains("src/lib.rs:1:pub struct Alpha;"));
}

#[test]
fn p28_filters_paths_from_current_directory() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());

    cli()
        .current_dir(dir.path())
        .args(["handle_value", "src/nested/mod.rs", "--fixed-strings"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "src/nested/mod.rs:2:fn handle_value() { println!(\"beta\"); }",
        ))
        .stdout(predicate::str::contains("src/lib.rs").not());
}

#[test]
fn p28_stats_go_to_stderr_while_hits_stay_on_stdout() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());

    cli()
        .args(["debug", "build", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let output = output({
        let mut command = cli();
        command
            .current_dir(dir.path())
            .args(["Alpha", "--fixed-strings", "--stats"]);
        command
    });

    assert!(output.status.success());
    let stdout = stdout_text(&output);
    let stderr = stderr_text(&output);
    assert!(stdout.contains("src/lib.rs:1:pub struct Alpha;"));
    assert!(!stdout.contains("backend="));
    assert!(stderr.contains("p28_ms="));
    assert!(stderr.contains("transport="));
    assert!(stderr.contains("backend="));
}

#[test]
fn p28_handles_anchored_line_start_regexes() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "fn build() {\n    SearchRequest {\n        query: pattern,\n    };\n}\n",
    )
    .unwrap();

    cli()
        .args(["debug", "build", dir.path().to_str().unwrap()])
        .assert()
        .success();

    cli()
        .current_dir(dir.path())
        .args([r"^\s*SearchRequest\s*\{", "--stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "src/main.rs:2:    SearchRequest {",
        ))
        .stderr(predicate::str::contains("backend="));
}

#[test]
fn debug_bench_prints_packet28_and_legacy_timings() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path());

    cli()
        .args(["debug", "build", dir.path().to_str().unwrap()])
        .assert()
        .success();

    cli()
        .args([
            "debug",
            "bench",
            dir.path().to_str().unwrap(),
            "Alpha",
            "--fixed-strings",
            "--transport",
            "inproc",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("guard=index"))
        .stdout(predicate::str::contains("parity=exact"))
        .stdout(predicate::str::contains("p28_ms="))
        .stdout(predicate::str::contains("legacy_rg_ms="));
}
