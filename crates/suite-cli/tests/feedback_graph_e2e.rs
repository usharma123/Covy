use assert_cmd::Command;
use predicates::prelude::*;
use rusqlite::Connection;
use std::fs;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_feedback_graph_cli_use_sqlite() {
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
    let transcript_fts_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM transcript_messages_fts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(transcript_fts_rows, 2);

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
