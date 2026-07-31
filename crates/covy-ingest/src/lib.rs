//! Coverage and diagnostics ingestion with explicit or detected formats.
//!
//! Path-based entry points read one file and either detect its format or use a
//! caller-selected parser. Reader entry points are useful for stdin, in-memory
//! fixtures, and other streams where no meaningful filename exists.
//!
//! # Coverage from a reader
//!
//! ```
//! use covy_core::CoverageFormat;
//! use covy_ingest::ingest_reader;
//!
//! let report = b"TN:example\nSF:src/lib.rs\nDA:1,1\nend_of_record\n";
//! let coverage = ingest_reader(&report[..], CoverageFormat::Lcov)
//!     .expect("valid LCOV report");
//!
//! assert!(coverage.files["src/lib.rs"].lines_covered.contains(1));
//! ```
//!
//! Auto-detection deliberately uses a bounded prefix and never guesses after
//! an unknown signature. Call an explicit-format entry point when input has an
//! unconventional filename or preamble.

/// Cobertura XML coverage ingestion.
pub mod cobertura;
/// Go cover-profile ingestion.
pub mod gocov;
/// JaCoCo XML coverage ingestion.
pub mod jacoco;
/// LCOV tracefile ingestion.
pub mod lcov;
/// LLVM coverage JSON ingestion.
pub mod llvmcov;
/// SARIF diagnostics ingestion.
pub mod sarif;

use std::path::Path;

use covy_core::diagnostics::{DiagnosticsData, DiagnosticsFormat};
use covy_core::model::{CoverageData, CoverageFormat};
use covy_core::CovyError;

/// Trait for coverage format parsers.
pub trait Ingestor: Send + Sync {
    /// Returns the format accepted by this parser.
    fn format(&self) -> CoverageFormat;

    /// Parses a complete coverage report from `data`.
    ///
    /// # Errors
    ///
    /// Returns [`CovyError`] when the report is empty, malformed, or contains
    /// values that cannot be represented by the normalized coverage model.
    fn parse(&self, data: &[u8]) -> Result<CoverageData, CovyError>;
}

/// Detect format from file extension and content sniffing.
///
/// Only the first 512 bytes are inspected after filename checks.
///
/// # Errors
///
/// Returns [`CovyError::UnknownFormat`] when neither the filename nor the
/// bounded content prefix identifies a supported coverage format.
pub fn detect_format(path: &Path, content: &[u8]) -> Result<CoverageFormat, CovyError> {
    // Check extension first
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if ext == "info" {
            return Ok(CoverageFormat::Lcov);
        }
    }

    // Check filename
    let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");

    if filename == "lcov.info" || filename.ends_with(".lcov") {
        return Ok(CoverageFormat::Lcov);
    }

    // Content sniffing
    let prefix = std::str::from_utf8(&content[..content.len().min(512)]).unwrap_or("");

    if prefix.starts_with("TN:") || prefix.starts_with("SF:") || prefix.contains("\nSF:") {
        return Ok(CoverageFormat::Lcov);
    }

    if prefix.starts_with("mode:") {
        return Ok(CoverageFormat::GoCov);
    }

    if prefix.contains("<coverage") || prefix.contains("<cobertura") {
        return Ok(CoverageFormat::Cobertura);
    }

    if prefix.contains("<!DOCTYPE report") || prefix.contains("<report") {
        return Ok(CoverageFormat::JaCoCo);
    }

    if prefix.contains("\"type\"") && prefix.contains("llvm.coverage.json.export") {
        return Ok(CoverageFormat::LlvmCov);
    }

    // Also detect by structure: { "data": [{ "files": ...
    if prefix.trim_start().starts_with('{')
        && prefix.contains("\"data\"")
        && prefix.contains("\"files\"")
    {
        return Ok(CoverageFormat::LlvmCov);
    }

    Err(CovyError::UnknownFormat {
        path: path.display().to_string(),
    })
}

/// Get the appropriate ingestor for a format.
pub fn get_ingestor(format: CoverageFormat) -> Box<dyn Ingestor> {
    match format {
        CoverageFormat::Lcov => Box::new(lcov::LcovIngestor),
        CoverageFormat::Cobertura => Box::new(cobertura::CoberturaIngestor),
        CoverageFormat::JaCoCo => Box::new(jacoco::JaCoCoIngestor),
        CoverageFormat::GoCov => Box::new(gocov::GoCovIngestor),
        CoverageFormat::LlvmCov => Box::new(llvmcov::LlvmCovIngestor),
    }
}

/// Convenience: ingest a file, auto-detecting format.
///
/// # Errors
///
/// Returns [`CovyError`] when the file cannot be read, its format cannot be
/// detected, or the detected parser rejects its contents.
pub fn ingest_path(path: &Path) -> Result<CoverageData, CovyError> {
    let content = std::fs::read(path)?;
    let format = detect_format(path, &content)?;
    let ingestor = get_ingestor(format);
    ingestor.parse(&content)
}

/// Ingest a file with a specified format.
///
/// # Errors
///
/// Returns [`CovyError`] when the file cannot be read, is empty, or is invalid
/// for `format`.
pub fn ingest_path_with_format(
    path: &Path,
    format: CoverageFormat,
) -> Result<CoverageData, CovyError> {
    let content = std::fs::read(path)?;
    if content.is_empty() {
        return Err(CovyError::EmptyInput {
            path: path.display().to_string(),
        });
    }
    let ingestor = get_ingestor(format);
    ingestor.parse(&content)
}

/// Ingest coverage data from a reader (e.g. stdin) with a specified format.
///
/// # Errors
///
/// Returns [`CovyError`] when the reader fails, yields no bytes, or contains a
/// report that is invalid for `format`.
pub fn ingest_reader<R: std::io::Read>(
    mut reader: R,
    format: CoverageFormat,
) -> Result<CoverageData, CovyError> {
    let mut content = Vec::new();
    reader.read_to_end(&mut content)?;
    if content.is_empty() {
        return Err(CovyError::EmptyInput {
            path: "(stdin)".into(),
        });
    }
    let ingestor = get_ingestor(format);
    ingestor.parse(&content)
}

/// Detect diagnostics format from file extension and content sniffing.
///
/// Only the first 1,024 bytes are inspected after filename checks.
///
/// # Errors
///
/// Returns [`CovyError::UnknownFormat`] when neither the filename nor the
/// bounded content prefix identifies a supported diagnostics format.
pub fn detect_diagnostics_format(
    path: &Path,
    content: &[u8],
) -> Result<DiagnosticsFormat, CovyError> {
    let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");

    if filename.ends_with(".sarif") || filename.ends_with(".sarif.json") {
        return Ok(DiagnosticsFormat::Sarif);
    }

    let prefix = std::str::from_utf8(&content[..content.len().min(1024)]).unwrap_or("");
    if prefix.contains("\"$schema\"") && prefix.to_lowercase().contains("sarif") {
        return Ok(DiagnosticsFormat::Sarif);
    }

    Err(CovyError::UnknownFormat {
        path: path.display().to_string(),
    })
}

/// Convenience: ingest diagnostics file, auto-detecting format.
///
/// # Errors
///
/// Returns [`CovyError`] when the file cannot be read, its diagnostics format
/// cannot be detected, or the detected parser rejects its contents.
pub fn ingest_diagnostics_path(path: &Path) -> Result<DiagnosticsData, CovyError> {
    let content = std::fs::read(path)?;
    let format = detect_diagnostics_format(path, &content)?;
    parse_diagnostics(&content, format)
}

/// Ingest diagnostics data from a reader with a specified format.
///
/// # Errors
///
/// Returns [`CovyError`] when the reader fails, yields no bytes, or contains a
/// report that is invalid for `format`.
pub fn ingest_diagnostics_reader<R: std::io::Read>(
    mut reader: R,
    format: DiagnosticsFormat,
) -> Result<DiagnosticsData, CovyError> {
    let mut content = Vec::new();
    reader.read_to_end(&mut content)?;
    parse_diagnostics(&content, format)
}

fn parse_diagnostics(
    content: &[u8],
    format: DiagnosticsFormat,
) -> Result<DiagnosticsData, CovyError> {
    if content.is_empty() {
        return Err(CovyError::EmptyInput {
            path: "(stdin)".into(),
        });
    }

    match format {
        DiagnosticsFormat::Sarif => sarif::parse_sarif(content),
    }
}
