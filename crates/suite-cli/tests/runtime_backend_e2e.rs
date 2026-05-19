use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{json, Value};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::OnceLock;
use std::time::Duration;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

fn ensure_packet28d_built() {
    static BUILT: OnceLock<()> = OnceLock::new();
    BUILT.get_or_init(|| {
        let status = std::process::Command::new("cargo")
            .args(["build", "-p", "packet28d"])
            .status()
            .unwrap();
        assert!(status.success(), "failed to build packet28d");
    });
}

#[cfg(not(target_os = "linux"))]
#[test]
fn test_runtime_backend_cli_shell_command_reports_linux_only_support_on_macos() {
    let dir = tempfile::tempdir().unwrap();
    suite_cmd()
        .current_dir(dir.path())
        .args(["shell", "--root", "."])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Packet28 shell is only supported on Linux in Phase A",
        ));
}

#[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
#[test]
fn test_runtime_backend_cli_run_command_auto_backend_reports_missing_platform_backend() {
    let dir = tempfile::tempdir().unwrap();
    suite_cmd()
        .current_dir(dir.path())
        .args(["run", "--root", ".", "--", "sh", "-c", "printf ok"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Packet28 run --backend linux-oci is not implemented yet",
        ));
}

#[cfg(target_os = "macos")]
#[test]
fn test_runtime_backend_cli_run_command_auto_backend_swaps_instruction_file_and_restores_it() {
    ensure_packet28d_built();
    let dir = tempfile::tempdir().unwrap();
    let original = format!(
        "# Large AGENTS\n\n{}\n",
        (0..120)
            .map(|idx| format!(
                "## Section {idx}\nPacket28 should compress repeated instruction text while keeping task aware guidance."
            ))
            .collect::<Vec<_>>()
            .join("\n\n")
    );
    fs::write(dir.path().join("AGENTS.md"), &original).unwrap();

    let claude = dir.path().join("claude");
    fs::write(
        &claude,
        "#!/bin/sh\nprintf '%s|%s\\n' \"$PACKET28_RUNTIME_BACKEND\" \"$PACKET28_AGENT_FAMILY\"\ncat AGENTS.md\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&claude).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&claude, perms).unwrap();
    }

    let output = suite_cmd()
        .current_dir(dir.path())
        .args(["run", "--root", ".", "--", claude.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&output);
    assert!(stdout.contains("macos_swap|claude"));
    assert!(stdout.contains("# [p28:virtual] sha256:"));
    assert_eq!(
        fs::read_to_string(dir.path().join("AGENTS.md")).unwrap(),
        original
    );

    let reports = fs::read_dir(dir.path().join(".packet28/runtime/macos-swap"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(reports.len(), 1);
    let report: Value = serde_json::from_slice(&fs::read(&reports[0]).unwrap()).unwrap();
    assert_eq!(
        report.get("state").and_then(Value::as_str),
        Some("restored")
    );
    assert_eq!(
        report.get("backend_kind").and_then(Value::as_str),
        Some("macos_swap")
    );
    let files = report.get("files").and_then(Value::as_array).unwrap();
    assert!(files.iter().any(|item| {
        item.get("path").and_then(Value::as_str) == Some("AGENTS.md")
            && item.get("decision").and_then(Value::as_str) == Some("rewrite")
    }));
}

#[cfg(target_os = "macos")]
#[test]
fn test_runtime_backend_cli_run_command_recovers_stale_macos_swap_session_before_launch() {
    ensure_packet28d_built();
    let dir = tempfile::tempdir().unwrap();
    let original = "tiny original\n";
    let swapped = "# [p28:virtual] stale\n";
    let agents = dir.path().join("AGENTS.md");
    fs::write(&agents, swapped).unwrap();
    let backup = dir.path().join("AGENTS.md.p28-backup.demo");
    let temp = dir.path().join("AGENTS.md.p28-rewrite.demo.tmp");
    fs::write(&backup, original).unwrap();
    fs::write(&temp, "temp").unwrap();
    let report_dir = dir.path().join(".packet28/runtime/macos-swap");
    fs::create_dir_all(&report_dir).unwrap();
    fs::write(
        report_dir.join("demo.json"),
        serde_json::to_vec_pretty(&json!({
            "session_id":"demo",
            "workspace_root": dir.path(),
            "command":["claude"],
            "agent_family":"claude",
            "backend_kind":"macos_swap",
            "pid":999999u32,
            "started_at":1u64,
            "state":"active",
            "files":[{
                "path":"AGENTS.md",
                "decision":"rewrite",
                "reason":null,
                "content_sha256":"abc",
                "task_label":"default",
                "original_bytes":swapped.len(),
                "rewritten_bytes":swapped.len(),
                "backup_path":backup,
                "temp_path":temp
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let claude = dir.path().join("claude");
    fs::write(&claude, "#!/bin/sh\ncat AGENTS.md\n").unwrap();
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&claude).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&claude, perms).unwrap();
    }

    let output = suite_cmd()
        .current_dir(dir.path())
        .args(["run", "--root", ".", "--", claude.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&output);
    assert_eq!(stdout, original);
    assert_eq!(fs::read_to_string(&agents).unwrap(), original);

    let recovered: Value =
        serde_json::from_slice(&fs::read(report_dir.join("demo.json")).unwrap()).unwrap();
    assert_eq!(
        recovered.get("state").and_then(Value::as_str),
        Some("restored")
    );
}

#[cfg(target_os = "macos")]
#[test]
fn test_runtime_backend_cli_run_command_restores_files_after_sigterm() {
    ensure_packet28d_built();
    let dir = tempfile::tempdir().unwrap();
    let original = format!(
        "# Large AGENTS\n\n{}\n",
        (0..80)
            .map(|idx| format!(
                "## Section {idx}\nPacket28 should compress repeated instruction text while keeping task aware guidance."
            ))
            .collect::<Vec<_>>()
            .join("\n\n")
    );
    let agents = dir.path().join("AGENTS.md");
    fs::write(&agents, &original).unwrap();

    let claude = dir.path().join("claude");
    fs::write(
        &claude,
        "#!/bin/sh\nprintf '%s' \"$PACKET28_RUNTIME_BACKEND\" > child-backend.txt\nwhile true; do sleep 1; done\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&claude).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&claude, perms).unwrap();
    }

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_Packet28"))
        .current_dir(dir.path())
        .args(["run", "--root", ".", "--", claude.to_str().unwrap()])
        .spawn()
        .unwrap();

    let report_dir = dir.path().join(".packet28/runtime/macos-swap");
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if report_dir.exists()
            && fs::read_dir(&report_dir)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| {
                    serde_json::from_slice::<Value>(&fs::read(entry.path()).unwrap())
                        .ok()
                        .and_then(|report| {
                            report
                                .get("state")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        })
                        .is_some_and(|state| state == "active")
                })
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let status = child.wait().unwrap();
    assert!(!status.success());
    let restore_start = std::time::Instant::now();
    loop {
        if fs::read_to_string(&agents).ok().as_deref() == Some(original.as_str()) {
            break;
        }
        assert!(
            restore_start.elapsed() < Duration::from_secs(3),
            "timed out waiting for AGENTS.md to be restored"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn test_runtime_backend_cli_shell_command_injects_ld_preload_for_explicit_commands() {
    ensure_packet28d_built();
    let dir = tempfile::tempdir().unwrap();
    let status = std::process::Command::new("cargo")
        .args(["build", "-p", "context-instruct-shim"])
        .status()
        .unwrap();
    assert!(status.success(), "failed to build context-instruct-shim");

    let output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "shell",
            "--root",
            ".",
            "--",
            "sh",
            "-c",
            "printf '%s' \"$LD_PRELOAD\"",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&output);
    assert!(stdout.contains("libcontext_instruct_shim.so"));
}

#[cfg(target_os = "linux")]
#[test]
fn test_runtime_backend_cli_run_command_linux_preload_sets_backend_and_agent_family() {
    ensure_packet28d_built();
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join("claude");
    fs::write(
        &claude,
        "#!/bin/sh\nprintf '%s|%s|%s' \"$LD_PRELOAD\" \"$PACKET28_RUNTIME_BACKEND\" \"$PACKET28_AGENT_FAMILY\"\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&claude).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&claude, perms).unwrap();
    }
    let status = std::process::Command::new("cargo")
        .args(["build", "-p", "context-instruct-shim"])
        .status()
        .unwrap();
    assert!(status.success(), "failed to build context-instruct-shim");

    let output = suite_cmd()
        .current_dir(dir.path())
        .args([
            "run",
            "--root",
            ".",
            "--backend",
            "linux-preload",
            "--",
            claude.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&output);
    assert!(stdout.contains("libcontext_instruct_shim.so"));
    assert!(stdout.contains("linux_preload"));
    assert!(stdout.contains("claude"));
}
