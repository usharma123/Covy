use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_dashboard_local_product_metrics() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let trend_fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("docs")
        .join("context-anomalies")
        .join("history.jsonl");
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(root.path())
        .status()
        .unwrap();
    fs::write(root.path().join("tracked.txt"), "changed\n").unwrap();

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "run",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
            "git",
            "status",
            "--short",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "store", "dashboard memory"])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args(["transcript", "append", "dashboard transcript context"])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args(["feedback", "record", "dashboard", "shows feedback"])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "link", "Dashboard", "Packet28"])
        .assert()
        .success();
    let task_id = "task-dashboard-handoff";
    for (context_version, body) in [
        (
            "ctx-dashboard-1",
            "cargo test -p suite-cli dashboard_handoff_test $PACKET28_DASHBOARD_MISSING_ENV_12345",
        ),
        (
            "ctx-dashboard-2",
            "cargo test -p suite-cli dashboard_handoff_test",
        ),
        (
            "ctx-dashboard-3",
            "cargo test -p suite-cli dashboard_handoff_test $PACKET28_DASHBOARD_MISSING_ENV_12345",
        ),
    ] {
        let path =
            packet28_daemon_core::task_version_json_path(root.path(), task_id, context_version);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "context_version": context_version,
                "artifact_id": context_version,
                "brief": "## Task Objective\nDashboard handoff readiness.",
                "sections": [{
                    "id": "verification",
                    "title": "Verification",
                    "body": body
                }],
                "changed_paths_since_checkpoint": ["src/lib.rs"],
                "next_action_summary": "surface dashboard handoff readiness"
            }))
            .unwrap(),
        )
        .unwrap();
    }

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "dashboard",
            "--root",
            root.path().to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"commands_reduced\":1"))
        .stdout(predicate::str::contains("\"top_saved_routes\""))
        .stdout(predicate::str::contains("\"route\":\"run_reducer:git\""))
        .stdout(predicate::str::contains("\"memory_count\":1"))
        .stdout(predicate::str::contains("\"memory_topics\""))
        .stdout(predicate::str::contains("\"topic\":\"general\""))
        .stdout(predicate::str::contains("\"memory_health\""))
        .stdout(predicate::str::contains("\"total_memories\":1"))
        .stdout(predicate::str::contains("\"feedback_corrections\":1"))
        .stdout(predicate::str::contains("\"feedback_stats\""))
        .stdout(predicate::str::contains("\"transcript_stats\""))
        .stdout(predicate::str::contains("\"message_count\":1"))
        .stdout(predicate::str::contains("\"graph_concepts\""))
        .stdout(predicate::str::contains("\"graph_stats\""))
        .stdout(predicate::str::contains("\"handoff_readiness\""))
        .stdout(predicate::str::contains("\"latest_status\":\"blocked\""))
        .stdout(predicate::str::contains(
            "\"latest_blocking_categories\":[\"environment\"]",
        ))
        .stdout(predicate::str::contains("\"regression_count\":1"))
        .stdout(predicate::str::contains("\"windsurf_doctor_status\""));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "dashboard",
            "--root",
            root.path().to_str().unwrap(),
            "--context-anomaly-history",
            trend_fixture.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"context_anomalies\""))
        .stdout(predicate::str::contains("\"latest_status\":\"ready\""))
        .stdout(predicate::str::contains(
            "\"recurring_hidden_categories\":[\"fallback_provenance\"]",
        ));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args(["dashboard", "--root", root.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("top_saved_routes=1"))
        .stdout(predicate::str::contains("memory_topics=1"))
        .stdout(predicate::str::contains("topics_needing_consolidation=0"))
        .stdout(predicate::str::contains("transcript_messages=1"))
        .stdout(predicate::str::contains("handoff_latest_status=blocked"))
        .stdout(predicate::str::contains("handoff_regression_count=1"));

    let html_path = root.path().join("packet28-dashboard.html");
    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "dashboard",
            "--root",
            root.path().to_str().unwrap(),
            "--format",
            "html",
            "--output",
            html_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("dashboard_html="));
    let html = fs::read_to_string(&html_path).unwrap();
    assert!(html.contains("<title>Packet28 Dashboard</title>"));
    assert!(html.contains("Saved tokens"));
    assert!(html.contains("Top Saved Routes"));
    assert!(html.contains("run_reducer:git"));
    assert!(html.contains("Memory Topics"));
    assert!(html.contains("Handoff Readiness"));
    assert!(html.contains("Integration Health"));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "dashboard",
            "--root",
            root.path().to_str().unwrap(),
            "--format",
            "tui",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Packet28 Dashboard"))
        .stdout(predicate::str::contains("panel=Overview"))
        .stdout(predicate::str::contains("top_saved_routes:"))
        .stdout(predicate::str::contains("commands_reduced=1"))
        .stdout(predicate::str::contains("handoff_regression_count=1"));

    suite_cmd()
        .current_dir(root.path())
        .env("HOME", home.path())
        .args([
            "dashboard",
            "--root",
            root.path().to_str().unwrap(),
            "--format",
            "tui",
            "--interactive",
        ])
        .write_stdin("memory\nintegrations\nq\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("panel=Memory"))
        .stdout(predicate::str::contains("recent_memories:"))
        .stdout(predicate::str::contains("panel=Integrations"))
        .stdout(predicate::str::contains("windsurf_doctor_status="));
}
