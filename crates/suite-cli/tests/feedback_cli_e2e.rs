#[path = "support/feedback_graph.rs"]
mod feedback_graph;

use feedback_graph::suite_cmd;
use predicates::prelude::*;
use rusqlite::Connection;
use tempfile::TempDir;

#[test]
fn test_feedback_cli_use_sqlite() {
    let home = TempDir::new().unwrap();

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "feedback",
            "record",
            "test subject",
            "prefer focused reducers",
            "--topic",
            "reducers",
            "--context",
            "test context",
            "--predicted",
            "verbose reducers",
            "--reason",
            "too noisy",
            "--source",
            "cli-test",
            "--project",
            "coverage-b",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("prefer focused reducers"))
        .stdout(predicate::str::contains("\"topic\":\"reducers\""))
        .stdout(predicate::str::contains("\"project\":\"coverage-b\""))
        .stdout(predicate::str::contains(
            "\"predicted\":\"verbose reducers\"",
        ));

    suite_cmd()
        .env("HOME", home.path())
        .args(["feedback", "search", "focused", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("prefer focused reducers"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["feedback", "list", "--topic", "reducers", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"topic\":\"reducers\""));
    suite_cmd()
        .env("HOME", home.path())
        .args(["feedback", "apply", "1", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"applied_count\":1"));

    let conn = Connection::open(home.path().join(".packet28").join("packet28.db")).unwrap();
    let feedback_fts_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM feedback_fts_all", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(feedback_fts_rows, 1);

    suite_cmd()
        .env("HOME", home.path())
        .args(["feedback", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"feedback_count\":1"))
        .stdout(predicate::str::contains("\"applied_count\":1"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["feedback", "delete", "1", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"deleted\":1"));
}
