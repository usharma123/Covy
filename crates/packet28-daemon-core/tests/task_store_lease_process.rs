#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::fs;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use packet28_daemon_core::task_store_lease::{
    acquire_task_store_writer_lease, try_acquire_task_store_retention_lease,
};
use tempfile::tempdir;

const CHILD_ENV: &str = "PACKET28_TEST_NESTED_LEASE_CHILD";
const ROOT_ENV: &str = "PACKET28_TEST_NESTED_LEASE_ROOT";

#[test]
fn nested_shared_lease_child_helper() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    let root = std::env::var_os(ROOT_ENV).expect("child workspace root");
    let root = Path::new(&root);
    let outer = acquire_task_store_writer_lease(root).expect("outer writer lease");
    let final_outer_clone = outer.clone();
    drop(outer);
    fs::write(root.join("inner-dropped"), b"ready").expect("publish child phase");
    let release = root.join("release-outer");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !release.exists() {
        assert!(
            Instant::now() < deadline,
            "parent did not release outer lease"
        );
        thread::sleep(Duration::from_millis(10));
    }
    drop(final_outer_clone);
}

#[test]
fn final_clone_preserves_cross_process_lock() {
    let root = tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("nested_shared_lease_child_helper")
        .arg("--nocapture")
        .env(CHILD_ENV, "1")
        .env(ROOT_ENV, root.path())
        .spawn()
        .unwrap();
    let ready = root.path().join("inner-dropped");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "lease helper exited before publishing its inner-drop phase"
        );
        assert!(
            Instant::now() < deadline,
            "lease helper did not become ready"
        );
        thread::sleep(Duration::from_millis(10));
    }

    assert!(
        try_acquire_task_store_retention_lease(root.path())
            .unwrap()
            .is_none(),
        "dropping an earlier clone released the child's final shared lease"
    );

    fs::write(root.path().join("release-outer"), b"release").unwrap();
    assert!(child.wait().unwrap().success());
    assert!(try_acquire_task_store_retention_lease(root.path())
        .unwrap()
        .is_some());
}
