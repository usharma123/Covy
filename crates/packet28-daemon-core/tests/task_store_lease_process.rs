#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use packet28_daemon_core::retention::{retain_task_store, RetentionOptions};
use packet28_daemon_core::storage::save_task_registry;
use packet28_daemon_core::task_store_lease::{
    acquire_daemon_instance_lease, acquire_daemon_task_store_lease,
    acquire_task_store_recovery_lease, acquire_task_store_writer_lease, daemon_instance_lock_path,
    try_acquire_task_store_retention_lease,
};
use packet28_daemon_core::DaemonCoreError;
use packet28_daemon_protocol::task::{TaskRecord, TaskRegistry};
use tempfile::tempdir;

const CHILD_ENV: &str = "PACKET28_TEST_NESTED_LEASE_CHILD";
const NESTED_WRITER_CHILD_ENV: &str = "PACKET28_TEST_NESTED_WRITER_CHILD";
const RECOVERY_EXIT_CHILD_ENV: &str = "PACKET28_TEST_RECOVERY_EXIT_CHILD";
const INSTANCE_GAP_CHILD_ENV: &str = "PACKET28_TEST_INSTANCE_GAP_CHILD";
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
fn nested_writer_child_helper() {
    if std::env::var_os(NESTED_WRITER_CHILD_ENV).is_none() {
        return;
    }
    let root = std::env::var_os(ROOT_ENV).expect("child workspace root");
    let root = Path::new(&root);
    let _outer = acquire_task_store_writer_lease(root).expect("outer writer lease");
    save_task_registry(
        root,
        &TaskRegistry {
            tasks: BTreeMap::from([(
                "nested-writer".to_string(),
                TaskRecord {
                    task_id: "nested-writer".to_string(),
                    ..TaskRecord::default()
                },
            )]),
        },
    )
    .expect("nested writer transaction");
    fs::write(root.join("inner-writer-dropped"), b"ready").expect("publish child phase");
    let release = root.join("release-nested-writer");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !release.exists() {
        assert!(
            Instant::now() < deadline,
            "parent did not release outer writer lease"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn recovery_lease_abrupt_exit_child_helper() {
    if std::env::var_os(RECOVERY_EXIT_CHILD_ENV).is_none() {
        return;
    }
    let root = std::env::var_os(ROOT_ENV).expect("child workspace root");
    let root = Path::new(&root);
    let _recovery = acquire_task_store_recovery_lease(root).expect("recovery lease");
    fs::write(root.join("recovery-owned"), b"ready").expect("publish recovery ownership");
    let release = root.join("exit-without-drop");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !release.exists() {
        assert!(
            Instant::now() < deadline,
            "parent did not request abrupt recovery exit"
        );
        thread::sleep(Duration::from_millis(10));
    }
    std::process::exit(86);
}

#[test]
fn instance_gap_child_helper() {
    if std::env::var_os(INSTANCE_GAP_CHILD_ENV).is_none() {
        return;
    }
    let root = std::env::var_os(ROOT_ENV).expect("child workspace root");
    let root = Path::new(&root);
    let _instance = acquire_daemon_instance_lease(root).expect("daemon instance lease");
    fs::write(root.join("instance-owned"), b"ready").expect("publish instance ownership");
    let acquire_shared = root.join("acquire-shared");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !acquire_shared.exists() {
        assert!(
            Instant::now() < deadline,
            "parent did not request daemon shared lease"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let _shared = acquire_daemon_task_store_lease(root).expect("daemon shared lifecycle lease");
    fs::write(root.join("shared-owned"), b"ready").expect("publish shared ownership");
    let release = root.join("release-instance-gap");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !release.exists() {
        assert!(
            Instant::now() < deadline,
            "parent did not release instance-gap child"
        );
        thread::sleep(Duration::from_millis(10));
    }
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

#[test]
fn nested_writer_transaction_does_not_release_outer_process_lease() {
    let root = tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("nested_writer_child_helper")
        .arg("--nocapture")
        .env(NESTED_WRITER_CHILD_ENV, "1")
        .env(ROOT_ENV, root.path())
        .spawn()
        .unwrap();
    let ready = root.path().join("inner-writer-dropped");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "nested writer helper exited before publishing its inner-drop phase"
        );
        assert!(
            Instant::now() < deadline,
            "nested writer helper did not become ready"
        );
        thread::sleep(Duration::from_millis(10));
    }

    assert!(
        try_acquire_task_store_retention_lease(root.path())
            .unwrap()
            .is_none(),
        "dropping the nested writer guard released the outer process lease"
    );

    fs::write(root.path().join("release-nested-writer"), b"release").unwrap();
    assert!(child.wait().unwrap().success());
    assert!(try_acquire_task_store_retention_lease(root.path())
        .unwrap()
        .is_some());
}

#[test]
fn operating_system_releases_recovery_lease_after_abrupt_process_exit() {
    let root = tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("recovery_lease_abrupt_exit_child_helper")
        .arg("--nocapture")
        .env(RECOVERY_EXIT_CHILD_ENV, "1")
        .env(ROOT_ENV, root.path())
        .spawn()
        .unwrap();
    let ready = root.path().join("recovery-owned");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "recovery helper exited before publishing ownership"
        );
        assert!(
            Instant::now() < deadline,
            "recovery helper did not become ready"
        );
        thread::sleep(Duration::from_millis(10));
    }

    assert!(
        try_acquire_task_store_retention_lease(root.path())
            .unwrap()
            .is_none(),
        "recovery helper did not own the exclusive lifecycle lease"
    );

    fs::write(root.path().join("exit-without-drop"), b"exit").unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(86));
    assert!(try_acquire_task_store_retention_lease(root.path())
        .unwrap()
        .is_some());
}

#[test]
fn retention_cannot_mutate_in_daemon_lifecycle_conversion_window() {
    let root = tempdir().unwrap();
    let artifact = root.path().join(".packet28/task/conversion-window");
    fs::create_dir_all(&artifact).unwrap();
    fs::write(artifact.join("payload.bin"), b"retain").unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("instance_gap_child_helper")
        .arg("--nocapture")
        .env(INSTANCE_GAP_CHILD_ENV, "1")
        .env(ROOT_ENV, root.path())
        .spawn()
        .unwrap();
    let instance_owned = root.path().join("instance-owned");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !instance_owned.exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "instance-gap helper exited before owning the instance gate"
        );
        assert!(
            Instant::now() < deadline,
            "instance-gap helper did not acquire the instance gate"
        );
        thread::sleep(Duration::from_millis(10));
    }

    let error = retain_task_store(
        root.path(),
        100,
        RetentionOptions::dry_run(None, Some(0)).apply(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        DaemonCoreError::RetentionBlockedByDaemon { path }
            if path == daemon_instance_lock_path(&fs::canonicalize(root.path()).unwrap())
    ));
    assert!(artifact.join("payload.bin").exists());
    assert!(!root.path().join(".packet28/.retention-trash").exists());

    fs::write(root.path().join("acquire-shared"), b"go").unwrap();
    let shared_owned = root.path().join("shared-owned");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !shared_owned.exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "instance-gap helper exited before acquiring shared lifecycle"
        );
        assert!(
            Instant::now() < deadline,
            "instance-gap helper did not acquire shared lifecycle"
        );
        thread::sleep(Duration::from_millis(10));
    }
    fs::write(root.path().join("release-instance-gap"), b"release").unwrap();
    assert!(child.wait().unwrap().success());
}
