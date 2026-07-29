use std::error::Error as _;
use std::io::{self, Cursor};

use packet28_daemon_core::integrity::compute_hash;
use packet28_daemon_core::retention::{
    inspect_task_store, RetentionMode, TASK_STORE_REPORT_SCHEMA_VERSION,
};
use packet28_daemon_core::trust::{load_trust_store, save_trust_store, TrustStore};
use packet28_daemon_core::{
    read_socket_message, DaemonCoreError, DaemonRequest, Result as DaemonCoreResult,
};
use packet28_daemon_protocol::frame::FrameError;

fn assert_public_error_bounds<T: std::error::Error + Send + Sync + 'static>() {}

#[test]
fn typed_error_satisfies_library_and_task_boundaries() {
    assert_public_error_bounds::<DaemonCoreError>();
}

#[test]
fn public_result_supports_a_trust_store_roundtrip() -> DaemonCoreResult<()> {
    let directory = tempfile::tempdir().expect("temporary directory should be created");
    let path = directory.path().join("trust.json");

    save_trust_store(&path, &TrustStore::default())?;
    let loaded = load_trust_store(&path)?;

    assert!(loaded.entries.is_empty());
    Ok(())
}

#[test]
fn malformed_persisted_json_retains_operation_and_path_context() {
    let directory = tempfile::tempdir().expect("temporary directory should be created");
    let path = directory.path().join("trust.json");
    std::fs::write(&path, "{not-json").expect("malformed fixture should be written");

    let error = load_trust_store(&path).expect_err("malformed JSON should fail");
    let DaemonCoreError::Json {
        operation,
        path: error_path,
        ..
    } = &error
    else {
        panic!("expected JSON error, got {error:?}");
    };

    assert_eq!(
        (*operation, error_path.as_path()),
        ("failed to decode trust store from", path.as_path())
    );
}

#[test]
fn malformed_persisted_json_preserves_serde_source() {
    let directory = tempfile::tempdir().expect("temporary directory should be created");
    let path = directory.path().join("trust.json");
    std::fs::write(&path, "{not-json").expect("malformed fixture should be written");

    let error = load_trust_store(&path).expect_err("malformed JSON should fail");

    assert!(error
        .source()
        .and_then(|source| source.downcast_ref::<serde_json::Error>())
        .is_some());
}

#[test]
fn malformed_persisted_json_provides_recovery_guidance() {
    let directory = tempfile::tempdir().expect("temporary directory should be created");
    let path = directory.path().join("trust.json");
    std::fs::write(&path, "{not-json").expect("malformed fixture should be written");

    let error = load_trust_store(&path).expect_err("malformed JSON should fail");

    assert_eq!(
        error.hint(),
        "Repair or regenerate the reported daemon state file before retrying."
    );
}

#[test]
fn filesystem_failure_preserves_io_source() {
    let directory = tempfile::tempdir().expect("temporary directory should be created");
    let missing = directory.path().join("missing-hook");

    let error = compute_hash(&missing).expect_err("missing hook should fail");
    let kind = error
        .source()
        .and_then(|source| source.downcast_ref::<io::Error>())
        .map(io::Error::kind);

    assert_eq!(kind, Some(io::ErrorKind::NotFound));
}

#[test]
fn compatibility_frame_failure_preserves_protocol_source() {
    let mut empty_frame = Cursor::new(0_u64.to_be_bytes());

    let error = read_socket_message::<_, DaemonRequest>(&mut empty_frame)
        .expect_err("zero-length frame should fail");
    let source = error
        .source()
        .and_then(|source| source.downcast_ref::<FrameError>());

    assert!(matches!(source, Some(FrameError::Empty)));
}

#[test]
fn public_task_store_inspection_is_timestamped_and_non_mutating() -> DaemonCoreResult<()> {
    let directory = tempfile::tempdir().expect("temporary directory should be created");

    let report = inspect_task_store(directory.path(), 123)?;

    assert_eq!(
        (
            report.schema_version,
            report.observed_at_unix,
            report.mode,
            report.metrics_before.task_registry_records,
            directory.path().join(".packet28").exists(),
        ),
        (
            TASK_STORE_REPORT_SCHEMA_VERSION,
            123,
            RetentionMode::Inspect,
            0,
            false,
        )
    );
    Ok(())
}
