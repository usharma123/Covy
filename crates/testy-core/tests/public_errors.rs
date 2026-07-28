use std::error::Error as _;
use std::io::ErrorKind;
use std::path::Path;

use testy_core::error::{AdapterError, AdapterResult, TestyError};
use testy_core::merge::merge_coverage_inputs;
use testy_core::pipeline::{ImpactAdapters, ImpactError};
use testy_core::pipeline_shard::ShardError;
use testy_core::pipeline_testmap::{
    build_testmap_artifacts, load_manifest_records, load_testmap, TestMapAdapters, TestMapError,
    TestMapManifestRecord,
};

#[derive(Debug, thiserror::Error)]
#[error("fixture adapter rejected the report")]
struct FixtureAdapterError;

fn failing_ingest(_path: &Path) -> AdapterResult<testy_core::model::CoverageData> {
    Err(AdapterError::external(
        "fixture coverage ingest",
        FixtureAdapterError,
    ))
}

fn assert_public_error<E: std::error::Error + Send + Sync + 'static>() {}

#[test]
fn public_error_aliases_are_typed_library_errors() {
    assert_public_error::<TestyError>();
    assert_public_error::<ImpactError>();
    assert_public_error::<ShardError>();
    assert_public_error::<TestMapError>();
}

#[test]
fn adapter_callbacks_use_the_typed_adapter_result() {
    let adapters = TestMapAdapters {
        ingest_coverage: failing_ingest,
    };
    let callback: fn(&Path) -> AdapterResult<testy_core::model::CoverageData> =
        adapters.ingest_coverage;

    let error = callback(Path::new("coverage.info")).expect_err("fixture adapter must fail");

    assert!(matches!(error, AdapterError::External { .. }));
}

#[test]
fn missing_testmap_preserves_io_source_and_path() {
    let directory = tempfile::tempdir().expect("create test directory");
    let path = directory.path().join("missing-testmap.bin");

    let error = load_testmap(&path).expect_err("missing testmap must fail");

    let TestyError::Io {
        operation,
        path: error_path,
        source,
    } = &error
    else {
        panic!("expected typed I/O error, got {error:?}");
    };
    assert_eq!(*operation, "Failed to read testmap at");
    assert_eq!(error_path, &path);
    assert_eq!(source.kind(), ErrorKind::NotFound);
    assert_eq!(
        error
            .source()
            .and_then(|source| source.downcast_ref::<std::io::Error>())
            .map(std::io::Error::kind),
        Some(ErrorKind::NotFound)
    );
}

#[test]
fn invalid_manifest_preserves_serde_json_source() {
    let directory = tempfile::tempdir().expect("create test directory");
    let path = directory.path().join("manifest.jsonl");
    std::fs::write(&path, "{\"test_id\":").expect("write deliberately malformed manifest fixture");

    let error = load_manifest_records(&[path]).expect_err("malformed JSON must fail");

    assert!(matches!(&error, TestyError::Json { .. }));
    assert!(error
        .source()
        .and_then(|source| source.downcast_ref::<serde_json::Error>())
        .is_some());
    assert!(error.to_string().contains("Expected JSONL shape"));
}

#[test]
fn corrupt_testmap_preserves_state_codec_source() {
    let directory = tempfile::tempdir().expect("create test directory");
    let path = directory.path().join("corrupt-testmap.bin");
    std::fs::write(&path, b"not-a-testmap").expect("write corrupt testmap fixture");

    let error = load_testmap(&path).expect_err("corrupt testmap must fail");

    assert!(matches!(&error, TestyError::State { .. }));
    assert!(error
        .source()
        .and_then(|source| source.downcast_ref::<testy_core::error::CovyError>())
        .is_some());
    assert_eq!(
        error.hint(),
        Some("Regenerate the test map or timing state from source inputs.")
    );
}

#[test]
fn strict_merge_preserves_the_failed_path_and_codec_source() {
    let directory = tempfile::tempdir().expect("create test directory");
    let path = directory.path().join("missing-coverage.bin");

    let error =
        merge_coverage_inputs(std::slice::from_ref(&path), true).expect_err("merge must fail");

    let TestyError::State {
        operation,
        path: error_path,
        source,
    } = &error
    else {
        panic!("expected typed state error, got {error:?}");
    };
    assert_eq!(*operation, "Failed to merge coverage input");
    assert_eq!(error_path, &path);
    assert!(matches!(
        source,
        testy_core::error::CovyError::IoRaw(io_error)
            if io_error.kind() == ErrorKind::NotFound
    ));
    assert!(error
        .source()
        .and_then(|source| source.downcast_ref::<testy_core::error::CovyError>())
        .is_some());
}

#[test]
fn adapter_failure_keeps_original_source_available() {
    let records = [TestMapManifestRecord {
        test_id: "example::test".to_string(),
        language: Some("python".to_string()),
        duration_ms: None,
        coverage_report: Some("coverage.info".to_string()),
        coverage_reports: Vec::new(),
    }];
    let adapters = TestMapAdapters {
        ingest_coverage: failing_ingest,
    };

    let error = build_testmap_artifacts(&records, &adapters)
        .err()
        .expect("adapter failure must propagate");

    let adapter = error
        .source()
        .and_then(|source| source.downcast_ref::<AdapterError>())
        .expect("top-level error must retain the adapter error");
    assert!(adapter
        .source()
        .and_then(|source| source.downcast_ref::<FixtureAdapterError>())
        .is_some());
    assert_eq!(
        error.hint(),
        Some("Inspect the nested adapter error for the original cause.")
    );
}

#[test]
fn impact_adapter_contract_is_fully_typed() {
    fn coverage(_path: &Path) -> AdapterResult<testy_core::model::CoverageData> {
        Ok(testy_core::model::CoverageData::new())
    }
    fn coverage_with_format(
        _path: &Path,
        _format: testy_core::model::CoverageFormat,
    ) -> AdapterResult<testy_core::model::CoverageData> {
        Ok(testy_core::model::CoverageData::new())
    }
    fn diff(_base: &str, _head: &str) -> AdapterResult<Vec<testy_core::model::FileDiff>> {
        Ok(Vec::new())
    }

    let adapters = ImpactAdapters {
        ingest_coverage_auto: coverage,
        ingest_coverage_with_format: coverage_with_format,
        git_diff: diff,
    };

    let callback: fn(&str, &str) -> AdapterResult<Vec<testy_core::model::FileDiff>> =
        adapters.git_diff;
    assert!(callback("base", "head").is_ok());
}
