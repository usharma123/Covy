use assert_cmd::Command;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

pub fn ensure_packet28d_built() {
    static BUILT: OnceLock<()> = OnceLock::new();
    BUILT.get_or_init(|| {
        let status = std::process::Command::new("cargo")
            .args(["build", "-p", "packet28d"])
            .status()
            .unwrap();
        assert!(status.success(), "failed to build packet28d");
    });
}

#[cfg(unix)]
pub fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

pub fn large_agents_text(sections: usize) -> String {
    format!(
        "# Large AGENTS\n\n{}\n",
        (0..sections)
            .map(|idx| format!(
                "## Section {idx}\nPacket28 should compress repeated instruction text while keeping task aware guidance."
            ))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

pub fn write_executable_script(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    make_executable(path);
}

pub fn swap_reports(report_dir: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(report_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}

pub fn wait_for_active_swap_report(report_dir: &Path, timeout: Duration) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if report_dir.exists()
            && std::fs::read_dir(report_dir)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| {
                    serde_json::from_slice::<serde_json::Value>(
                        &std::fs::read(entry.path()).unwrap(),
                    )
                    .ok()
                    .and_then(|report| {
                        report
                            .get("state")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    })
                    .is_some_and(|state| state == "active")
                })
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
