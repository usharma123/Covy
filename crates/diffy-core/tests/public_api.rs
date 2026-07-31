use std::error::Error;
use std::path::Path;

use diffy_core::diagnostics::DiagnosticsData;
use diffy_core::diff::{git_diff, parse_diff_output};
use diffy_core::error::{CovyError, DiffyError};
use diffy_core::model::{CoverageData, CoverageFormat, FileDiff};
use diffy_core::pipeline::{run_analysis, PipelineIngestAdapters, PipelineOutput, PipelineRequest};

fn ingest_coverage_auto(_path: &Path) -> std::result::Result<CoverageData, CovyError> {
    Ok(CoverageData::new())
}

fn ingest_coverage_with_format(
    _path: &Path,
    _format: CoverageFormat,
) -> std::result::Result<CoverageData, CovyError> {
    Ok(CoverageData::new())
}

fn ingest_coverage_stdin(_format: CoverageFormat) -> std::result::Result<CoverageData, CovyError> {
    Ok(CoverageData::new())
}

fn ingest_diagnostics(_path: &Path) -> std::result::Result<DiagnosticsData, CovyError> {
    Ok(DiagnosticsData::new())
}

#[test]
fn diff_entrypoints_expose_diffy_error() {
    let _: fn(&str, &str) -> std::result::Result<Vec<FileDiff>, DiffyError> = git_diff;
    let _: fn(&str) -> std::result::Result<Vec<FileDiff>, DiffyError> = parse_diff_output;
}

#[test]
fn pipeline_entrypoint_and_ports_expose_typed_errors() {
    let _: fn(
        PipelineRequest,
        &PipelineIngestAdapters,
    ) -> std::result::Result<PipelineOutput, DiffyError> = run_analysis;

    let adapters = PipelineIngestAdapters {
        ingest_coverage_auto,
        ingest_coverage_with_format,
        ingest_coverage_stdin,
        ingest_diagnostics,
    };

    let _: fn(&Path) -> std::result::Result<CoverageData, CovyError> =
        adapters.ingest_coverage_auto;
    let _: fn(&Path, CoverageFormat) -> std::result::Result<CoverageData, CovyError> =
        adapters.ingest_coverage_with_format;
    let _: fn(CoverageFormat) -> std::result::Result<CoverageData, CovyError> =
        adapters.ingest_coverage_stdin;
    let _: fn(&Path) -> std::result::Result<DiagnosticsData, CovyError> =
        adapters.ingest_diagnostics;
}

#[test]
fn diffy_error_is_external_error_boundary() {
    fn assert_error<T: Error + Send + Sync + 'static>() {}

    assert_error::<DiffyError>();
    let error = DiffyError::NoCoverageFiles;

    assert!(matches!(error, DiffyError::NoCoverageFiles));
}

#[test]
fn legacy_covy_error_reexport_remains_available() {
    let error: diffy_core::error::CovyError = CovyError::Other("compatibility check".to_string());

    assert_eq!(error.to_string(), "compatibility check");
}

#[test]
fn typed_error_is_available_beside_reexported_entrypoints() {
    let _: Option<diffy_core::diff::DiffyError> = None;
    let _: Option<diffy_core::pipeline::DiffyError> = None;
}
