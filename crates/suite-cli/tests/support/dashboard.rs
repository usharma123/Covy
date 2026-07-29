use assert_cmd::Command;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

pub fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

pub fn context_anomaly_history_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("docs")
        .join("context-anomalies")
        .join("history.jsonl")
}

pub fn seed_dashboard_product_state(root: &Path, home: &Path) {
    crate::process_harness::run_git(root, &["init"]);
    fs::write(root.join("tracked.txt"), "changed\n").unwrap();

    suite_cmd()
        .current_dir(root)
        .env("HOME", home)
        .args([
            "run",
            "--root",
            root.to_str().unwrap(),
            "--json",
            "git",
            "status",
            "--short",
        ])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home)
        .args(["memory", "store", "dashboard memory"])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home)
        .args(["transcript", "append", "dashboard transcript context"])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home)
        .args(["feedback", "record", "dashboard", "shows feedback"])
        .assert()
        .success();
    suite_cmd()
        .env("HOME", home)
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
            packet28_daemon_protocol::paths::task_version_json_path(root, task_id, context_version);
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
}
