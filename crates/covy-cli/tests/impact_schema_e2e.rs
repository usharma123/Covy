use assert_cmd::Command;
use predicates::prelude::*;

fn covy_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("covy")
}

#[test]
fn test_impact_and_shard_and_testmap_schema_flags() {
    covy_cmd()
        .args(["impact", "run", "--schema"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"tests\""))
        .stdout(predicate::str::contains("\"changed_lines_total\""))
        .stdout(predicate::str::contains("\"total_changed_lines\"").not());

    covy_cmd()
        .args(["impact", "record", "--schema"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"example_line\""));

    covy_cmd()
        .args(["shard", "plan", "--schema"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"tasks_json\""))
        .stdout(predicate::str::contains("\"impact_json\""));

    covy_cmd()
        .args(["testmap", "build", "--schema"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"test_id\""))
        .stdout(predicate::str::contains("\"coverage_report\""));
}
