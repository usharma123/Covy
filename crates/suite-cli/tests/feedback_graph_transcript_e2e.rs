#[path = "support/feedback_graph.rs"]
mod feedback_graph;

use feedback_graph::suite_cmd;
use predicates::prelude::*;
use rusqlite::Connection;
use tempfile::TempDir;

#[test]
fn test_feedback_graph_transcript_wakeup_filters_project_recall() {
    let home = TempDir::new().unwrap();

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "transcript",
            "append",
            "Need compact transcript recall for reducers",
            "--session",
            "cli-session",
            "--agent",
            "codex",
            "--role",
            "user",
            "--source",
            "cli-test",
            "--project",
            "coverage-b",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"session_key\":\"cli-session\""))
        .stdout(predicate::str::contains("\"role\":\"user\""))
        .stdout(predicate::str::contains("\"project\":\"coverage-b\""));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "transcript",
            "append",
            "Foreign transcript recall for reducers",
            "--session",
            "foreign-session",
            "--agent",
            "codex",
            "--role",
            "user",
            "--source",
            "cli-test",
            "--project",
            "coverage-foreign",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"project\":\"coverage-foreign\""));
    suite_cmd()
        .env("HOME", home.path())
        .args(["transcript", "search", "reducers", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("compact transcript recall"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["transcript", "show", "cli-session", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"agent\":\"codex\""));
    suite_cmd()
        .env("HOME", home.path())
        .args(["transcript", "list", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"message_count\":1"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["transcript", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"session_count\":2"))
        .stdout(predicate::str::contains("\"message_count\":2"));
    suite_cmd()
        .env("HOME", home.path())
        .args([
            "wakeup",
            "--query",
            "reducers",
            "--project",
            "coverage-b",
            "--format",
            "plain",
            "--max-tokens",
            "80",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\":\"packet28.wakeup.v1\""))
        .stdout(predicate::str::contains("\"format\":\"plain\""))
        .stdout(predicate::str::contains("\"estimated_tokens\""))
        .stdout(predicate::str::contains("\"transcripts\""))
        .stdout(predicate::str::contains("\"pack\""))
        .stdout(predicate::str::contains("compact transcript recall"))
        .stdout(predicate::str::contains("\"project\":\"coverage-b\""))
        .stdout(predicate::str::contains("Foreign transcript recall").not());

    let conn = Connection::open(home.path().join(".packet28").join("packet28.db")).unwrap();
    let transcript_fts_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM transcript_messages_fts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(transcript_fts_rows, 2);
}
