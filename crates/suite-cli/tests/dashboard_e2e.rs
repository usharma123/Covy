#[path = "support/dashboard.rs"]
mod dashboard;

use dashboard::{context_anomaly_history_fixture, seed_dashboard_product_state, suite_cmd};
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_dashboard_local_product_metrics() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let trend_fixture = context_anomaly_history_fixture();
    seed_dashboard_product_state(root.path(), home.path());

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
