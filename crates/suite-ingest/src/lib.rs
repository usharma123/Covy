//! Shared coverage and diagnostics ingestion helpers for suite-facing binaries.
//!
//! The helpers accept report paths, delegate format parsing to `covy_ingest`, and
//! merge multiple reports into the packet types used across the workspace.
//!
//! # Example
//!
//! ```
//! use std::time::{SystemTime, UNIX_EPOCH};
//!
//! use suite_ingest::ingest_coverage_path;
//!
//! let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
//! let path = std::env::temp_dir().join(format!(
//!     "suite-ingest-doctest-{}-{nonce}.info",
//!     std::process::id()
//! ));
//! std::fs::write(
//!     &path,
//!     "TN:\nSF:src/lib.rs\nDA:1,1\nend_of_record\n",
//! )?;
//!
//! let coverage = ingest_coverage_path(&path, None);
//! std::fs::remove_file(&path)?;
//! let coverage = coverage?;
//! assert!(coverage.files.contains_key("src/lib.rs"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::io::Read;
use std::path::Path;

use suite_packet_core::{CoverageData, CoverageFormat, CovyError, DiagnosticsData};

/// Ingest one coverage report, using `format` when supplied or auto-detecting it.
///
/// # Errors
///
/// Returns the underlying I/O, format-detection, or parser error.
pub fn ingest_coverage_path(
    path: &Path,
    format: Option<CoverageFormat>,
) -> Result<CoverageData, CovyError> {
    match format {
        Some(format) => covy_ingest::ingest_path_with_format(path, format),
        None => covy_ingest::ingest_path(path),
    }
}

/// Ingest and merge coverage reports in input order.
///
/// # Errors
///
/// Returns immediately when any input cannot be read, detected, or parsed.
pub fn ingest_coverage_paths(
    paths: &[String],
    format: Option<CoverageFormat>,
) -> Result<CoverageData, CovyError> {
    let mut merged = CoverageData::new();
    for path in paths {
        let data = ingest_coverage_path(Path::new(path), format)?;
        merged.merge(&data);
    }
    Ok(merged)
}

/// Ingest a coverage report from standard input using an explicit format.
///
/// # Errors
///
/// Returns the underlying standard-input I/O or parser error.
pub fn ingest_coverage_stdin(format: CoverageFormat) -> Result<CoverageData, CovyError> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .read_to_end(&mut bytes)
        .map_err(CovyError::IoRaw)?;
    covy_ingest::ingest_reader(bytes.as_slice(), format)
}

/// Ingest one diagnostics report, auto-detecting its format.
///
/// # Errors
///
/// Returns the underlying I/O, format-detection, or parser error.
pub fn ingest_diagnostics_path(path: &Path) -> Result<DiagnosticsData, CovyError> {
    covy_ingest::ingest_diagnostics_path(path)
}

/// Ingest and merge diagnostics reports, deduplicating issues by fingerprint.
///
/// # Errors
///
/// Returns immediately when any input cannot be read, detected, or parsed.
pub fn ingest_diagnostics_paths(paths: &[String]) -> Result<DiagnosticsData, CovyError> {
    let mut merged = DiagnosticsData::new();
    for path in paths {
        let data = ingest_diagnostics_path(Path::new(path))?;
        merged.merge(&data);
    }
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;
    use std::path::PathBuf;

    use suite_packet_core::{CoverageFormat, CovyError, DiagnosticsFormat};

    use super::{
        ingest_coverage_path, ingest_coverage_paths, ingest_diagnostics_path,
        ingest_diagnostics_paths,
    };

    fn fixture(relative: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("tests/fixtures")
            .join(relative)
    }

    fn path_string(path: &std::path::Path) -> String {
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn coverage_path_auto_detects_lcov() {
        let coverage =
            ingest_coverage_path(&fixture("lcov/basic.info"), None).expect("ingest LCOV fixture");

        assert_eq!(coverage.format, Some(CoverageFormat::Lcov));
        assert_eq!(coverage.files.len(), 2);
        assert!(coverage.files.contains_key("src/main.rs"));
        assert!(coverage.files.contains_key("src/lib.rs"));
    }

    #[test]
    fn coverage_path_honors_explicit_format() {
        let coverage = ingest_coverage_path(
            &fixture("cobertura/basic.xml"),
            Some(CoverageFormat::Cobertura),
        )
        .expect("ingest Cobertura fixture");

        assert_eq!(coverage.format, Some(CoverageFormat::Cobertura));
        assert!(!coverage.files.is_empty());
    }

    #[test]
    fn coverage_paths_merge_distinct_reports() {
        let paths = [
            path_string(&fixture("lcov/basic.info")),
            path_string(&fixture("gocov/basic.out")),
        ];

        let coverage = ingest_coverage_paths(&paths, None).expect("merge coverage fixtures");

        assert_eq!(coverage.files.len(), 4);
        assert!(coverage.files.contains_key("src/main.rs"));
        assert!(coverage.files.contains_key("main.go"));
    }

    #[test]
    fn coverage_path_preserves_missing_file_error() {
        let path = fixture("lcov/does-not-exist.info");

        let error = ingest_coverage_path(&path, None).expect_err("missing path must fail");

        assert!(matches!(
            error,
            CovyError::IoRaw(ref source) if source.kind() == ErrorKind::NotFound
        ));
    }

    #[test]
    fn diagnostics_paths_merge_and_deduplicate() {
        let path = path_string(&fixture("sarif/basic.sarif"));
        let diagnostics =
            ingest_diagnostics_paths(&[path.clone(), path]).expect("merge SARIF fixtures");

        assert_eq!(diagnostics.total_issues(), 5);
        assert_eq!(diagnostics.issues_by_file.len(), 2);
    }

    #[test]
    fn diagnostics_path_rejects_unknown_format() {
        let path = fixture("lcov/basic.info");

        let error = ingest_diagnostics_path(&path).expect_err("LCOV is not diagnostics data");

        assert!(matches!(
            error,
            CovyError::UnknownFormat { path: error_path }
                if error_path == path.display().to_string()
        ));
    }

    #[test]
    fn diagnostics_path_auto_detects_sarif() {
        let diagnostics =
            ingest_diagnostics_path(&fixture("sarif/basic.sarif")).expect("ingest SARIF fixture");

        assert_eq!(diagnostics.format, Some(DiagnosticsFormat::Sarif));
        assert_eq!(diagnostics.total_issues(), 5);
    }
}
