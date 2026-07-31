use std::collections::HashSet;
use std::path::{Path, PathBuf};

use suite_foundation_core::cache::DiagnosticsStateMetadata;

pub use crate::error::DiffyError;

use crate::config::GateConfig;
use crate::diagnostics::DiagnosticsData;
use crate::diff::git_diff;
use crate::error::{CovyError, Result};
use crate::gate::evaluate_full_gate;
use crate::model::{CoverageData, CoverageFormat, FileDiff, QualityGateResult};

/// Coverage input source selection for the diff pipeline.
#[derive(Debug, Clone)]
pub struct PipelineCoverageInput {
    /// Coverage report file paths (glob patterns allowed).
    pub paths: Vec<String>,
    /// Coverage format for report ingestion. `None` means auto-detect.
    pub format: Option<CoverageFormat>,
    /// Read coverage from stdin.
    pub stdin: bool,
    /// Explicit coverage state path (`--input` semantics).
    pub input_state_path: Option<String>,
    /// Fallback coverage state path when no explicit input or paths are provided.
    /// Set to `None` to disable fallback-to-state behavior.
    pub default_input_state_path: Option<String>,
    /// Prefixes to strip from ingested coverage paths.
    pub strip_prefixes: Vec<String>,
    /// Reject combining report paths with explicit state input.
    pub reject_paths_with_input: bool,
    /// Error text when there are no coverage inputs and state fallback is disabled.
    pub no_inputs_error: String,
}

/// Diagnostics input source selection for the diff pipeline.
#[derive(Debug, Clone)]
pub struct PipelineDiagnosticsInput {
    /// Diagnostics report file paths (glob patterns allowed).
    pub issue_patterns: Vec<String>,
    /// Explicit diagnostics state path.
    pub issues_state_path: Option<String>,
    /// Disable diagnostics state fallback when no diagnostics reports are provided.
    pub no_issues_state: bool,
    /// Default diagnostics state path when `issues_state_path` is not set.
    pub default_issues_state_path: String,
}

/// Canonical request for running diff analysis pipeline.
#[derive(Debug, Clone)]
pub struct PipelineRequest {
    pub base: String,
    pub head: String,
    pub source_root: Option<PathBuf>,
    pub coverage: PipelineCoverageInput,
    pub diagnostics: PipelineDiagnosticsInput,
    pub gate: GateConfig,
}

/// Ingest callbacks supplied by caller to avoid crate dependency cycles.
#[derive(Clone, Copy)]
pub struct PipelineIngestAdapters {
    /// Ingest a coverage report using format detection.
    pub ingest_coverage_auto: fn(&Path) -> std::result::Result<CoverageData, CovyError>,
    /// Ingest a coverage report using an explicit format.
    pub ingest_coverage_with_format:
        fn(&Path, CoverageFormat) -> std::result::Result<CoverageData, CovyError>,
    /// Ingest coverage from standard input using an explicit format.
    pub ingest_coverage_stdin: fn(CoverageFormat) -> std::result::Result<CoverageData, CovyError>,
    /// Ingest a diagnostics report using format detection.
    pub ingest_diagnostics: fn(&Path) -> std::result::Result<DiagnosticsData, CovyError>,
}

/// Diff-oriented context computed from base/head.
#[derive(Debug, Clone)]
pub struct ChangedLineContext {
    pub diffs: Vec<FileDiff>,
    pub changed_paths: HashSet<String>,
}

/// Fully analyzed pipeline output.
#[derive(Debug, Clone)]
pub struct PipelineOutput {
    pub coverage: CoverageData,
    pub diagnostics: Option<DiagnosticsData>,
    pub changed_line_context: ChangedLineContext,
    pub gate_result: QualityGateResult,
}

/// Run coverage and diagnostics analysis against a Git diff.
///
/// # Errors
///
/// Returns [`DiffyError`] when input selection is invalid, a report or cached
/// state cannot be loaded, a glob is invalid, or the requested Git diff cannot
/// be computed.
pub fn run_analysis(
    request: PipelineRequest,
    adapters: &PipelineIngestAdapters,
) -> Result<PipelineOutput> {
    let source_root = request.source_root.as_deref();

    let mut coverage = resolve_coverage_input(&request.coverage, adapters)?;
    suite_foundation_core::pathmap::auto_normalize_paths(&mut coverage, source_root);

    tracing::info!("Computing diff {}..{}", request.base, request.head);
    let diffs = git_diff(&request.base, &request.head)?;
    tracing::info!("Found {} changed files", diffs.len());
    let changed_paths: HashSet<String> = diffs.iter().map(|d| d.path.clone()).collect();

    let mut diagnostics =
        resolve_diagnostics_input(&request.diagnostics, &changed_paths, source_root, adapters)?;

    if let Some(diag) = diagnostics.data.as_mut() {
        if diagnostics.needs_normalization {
            suite_foundation_core::pathmap::auto_normalize_issue_paths(diag, source_root);
        }
    }

    let gate_result =
        evaluate_full_gate(&request.gate, &coverage, diagnostics.data.as_ref(), &diffs);

    Ok(PipelineOutput {
        coverage,
        diagnostics: diagnostics.data,
        changed_line_context: ChangedLineContext {
            diffs,
            changed_paths,
        },
        gate_result,
    })
}

fn resolve_coverage_input(
    input: &PipelineCoverageInput,
    adapters: &PipelineIngestAdapters,
) -> Result<CoverageData> {
    if input.stdin {
        if !input.paths.is_empty() {
            return Err(DiffyError::ConflictingInputs {
                first: "positional coverage paths",
                second: "--stdin",
            });
        }
        if input.input_state_path.is_some() {
            return Err(DiffyError::ConflictingInputs {
                first: "--input",
                second: "--stdin",
            });
        }
        let fmt = input.format.ok_or(DiffyError::MissingStdinFormat)?;
        return (adapters.ingest_coverage_stdin)(fmt)
            .map_err(|source| DiffyError::CoverageStdinIngest { source });
    }

    if !input.paths.is_empty() {
        if input.input_state_path.is_some() && input.reject_paths_with_input {
            return Err(DiffyError::ConflictingInputs {
                first: "positional coverage paths",
                second: "--input",
            });
        }
        return ingest_coverage_paths(&input.paths, input.format, &input.strip_prefixes, adapters);
    }

    if let Some(path) = input.input_state_path.as_deref() {
        let state_path = Path::new(path);
        if !state_path.exists() {
            return Err(DiffyError::CoverageStateNotFound {
                path: state_path.to_path_buf(),
            });
        }
        return load_coverage_state(state_path);
    }

    if let Some(path) = input.default_input_state_path.as_deref() {
        let state_path = Path::new(path);
        if !state_path.exists() {
            return Err(DiffyError::DefaultCoverageStateNotFound {
                path: state_path.to_path_buf(),
            });
        }
        return load_coverage_state(state_path);
    }

    Err(DiffyError::MissingCoverageInput {
        message: input.no_inputs_error.clone(),
    })
}

fn ingest_coverage_paths(
    patterns: &[String],
    format: Option<CoverageFormat>,
    strip_prefixes: &[String],
    adapters: &PipelineIngestAdapters,
) -> Result<CoverageData> {
    let files = resolve_globs(patterns, "coverage")?;
    if files.is_empty() {
        return Err(DiffyError::NoCoverageFiles);
    }

    let mut combined = CoverageData::new();
    for file in &files {
        tracing::info!("Ingesting {}", file.display());
        let data = match format {
            Some(fmt) => (adapters.ingest_coverage_with_format)(file, fmt),
            None => (adapters.ingest_coverage_auto)(file),
        }
        .map_err(|source| DiffyError::CoverageIngest {
            path: file.clone(),
            source,
        })?;
        let data = if strip_prefixes.is_empty() {
            data
        } else {
            apply_strip_prefixes(data, strip_prefixes)
        };
        combined.merge(&data);
    }
    Ok(combined)
}

fn load_coverage_state(path: &Path) -> Result<CoverageData> {
    tracing::info!("Loading coverage from state {}", path.display());
    let bytes = std::fs::read(path).map_err(|source| DiffyError::CoverageStateRead {
        path: path.to_path_buf(),
        source,
    })?;
    let data = suite_foundation_core::cache::deserialize_coverage(&bytes).map_err(|source| {
        DiffyError::CoverageStateDecode {
            path: path.to_path_buf(),
            source,
        }
    })?;
    Ok(data)
}

#[derive(Default)]
struct LoadedDiagnostics {
    data: Option<DiagnosticsData>,
    needs_normalization: bool,
}

impl LoadedDiagnostics {
    fn none() -> Self {
        Self {
            data: None,
            needs_normalization: false,
        }
    }
}

fn resolve_diagnostics_input(
    input: &PipelineDiagnosticsInput,
    selected_paths: &HashSet<String>,
    source_root: Option<&Path>,
    adapters: &PipelineIngestAdapters,
) -> Result<LoadedDiagnostics> {
    if !input.issue_patterns.is_empty() {
        let diagnostics = ingest_issues_patterns(&input.issue_patterns, adapters)?;
        return Ok(LoadedDiagnostics {
            data: Some(diagnostics),
            needs_normalization: true,
        });
    }

    if input.no_issues_state {
        return Ok(LoadedDiagnostics::none());
    }

    let state_path = input
        .issues_state_path
        .as_deref()
        .unwrap_or(&input.default_issues_state_path);
    let state_path = Path::new(state_path);
    if !state_path.exists() {
        return Ok(LoadedDiagnostics::none());
    }

    tracing::info!(
        "Loading diagnostics from cached state {}",
        state_path.display()
    );
    let (diagnostics, meta) =
        suite_foundation_core::cache::deserialize_diagnostics_for_paths_from_file(
            state_path,
            selected_paths,
        )
        .map_err(|source| DiffyError::DiagnosticsStateLoad {
            path: state_path.to_path_buf(),
            source,
        })?;
    let needs_normalization = !state_metadata_compatible(meta.as_ref(), source_root);
    Ok(LoadedDiagnostics {
        data: Some(diagnostics),
        needs_normalization,
    })
}

fn ingest_issues_patterns(
    patterns: &[String],
    adapters: &PipelineIngestAdapters,
) -> Result<DiagnosticsData> {
    let files = resolve_globs(patterns, "diagnostics")?;
    if files.is_empty() {
        return Err(DiffyError::NoDiagnosticsFiles);
    }

    let mut combined = DiagnosticsData::new();
    for file in &files {
        tracing::info!("Ingesting diagnostics {}", file.display());
        let data = load_diagnostics_input(file, adapters)?;
        combined.merge(&data);
    }
    Ok(combined)
}

fn load_diagnostics_input(
    path: &Path,
    adapters: &PipelineIngestAdapters,
) -> Result<DiagnosticsData> {
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("bin"))
    {
        let bytes = std::fs::read(path).map_err(|source| DiffyError::DiagnosticsStateRead {
            path: path.to_path_buf(),
            source,
        })?;
        let diagnostics =
            suite_foundation_core::cache::deserialize_diagnostics(&bytes).map_err(|source| {
                DiffyError::DiagnosticsStateDecode {
                    path: path.to_path_buf(),
                    source,
                }
            })?;
        return Ok(diagnostics);
    }

    (adapters.ingest_diagnostics)(path).map_err(|source| DiffyError::DiagnosticsIngest {
        path: path.to_path_buf(),
        source,
    })
}

fn resolve_globs(patterns: &[String], label: &str) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for pattern in patterns {
        let matches: Vec<_> = glob::glob(pattern)
            .map_err(|source| DiffyError::InvalidGlob {
                pattern: pattern.clone(),
                source,
            })?
            .filter_map(|r| r.ok())
            .collect();
        if matches.is_empty() {
            tracing::warn!("No {label} files matched pattern: {pattern}");
        }
        files.extend(matches);
    }
    Ok(files)
}

fn state_metadata_compatible(
    meta: Option<&DiagnosticsStateMetadata>,
    source_root: Option<&Path>,
) -> bool {
    let Some(meta) = meta else {
        return false;
    };

    if meta.schema_version != suite_foundation_core::cache::DIAGNOSTICS_STATE_SCHEMA_VERSION {
        return false;
    }
    if meta.path_norm_version != suite_foundation_core::cache::DIAGNOSTICS_PATH_NORM_VERSION {
        return false;
    }
    if !meta.normalized_paths {
        return false;
    }

    let current_root_id = suite_foundation_core::cache::current_repo_root_id(source_root);
    meta.repo_root_id == current_root_id
}

fn apply_strip_prefixes(data: CoverageData, prefixes: &[String]) -> CoverageData {
    let mut result = CoverageData {
        files: std::collections::BTreeMap::new(),
        format: data.format,
        timestamp: data.timestamp,
    };
    for (path, fc) in data.files {
        let mut stripped = path.as_str();
        for prefix in prefixes {
            if let Some(rest) = stripped.strip_prefix(prefix) {
                stripped = rest;
                break;
            }
        }
        result.files.insert(stripped.to_string(), fc);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    use crate::diagnostics::{Issue, Severity};
    use crate::model::FileCoverage;

    fn ingest_cov_auto_stub(_path: &Path) -> std::result::Result<CoverageData, CovyError> {
        Ok(make_coverage("src/from-paths.rs"))
    }

    fn ingest_cov_with_format_stub(
        _path: &Path,
        _format: CoverageFormat,
    ) -> std::result::Result<CoverageData, CovyError> {
        Ok(make_coverage("src/from-paths-format.rs"))
    }

    fn ingest_cov_stdin_stub(
        _format: CoverageFormat,
    ) -> std::result::Result<CoverageData, CovyError> {
        Ok(make_coverage("src/from-stdin.rs"))
    }

    fn ingest_diag_stub(_path: &Path) -> std::result::Result<DiagnosticsData, CovyError> {
        Ok(DiagnosticsData::new())
    }

    fn ingest_cov_auto_failure(_path: &Path) -> std::result::Result<CoverageData, CovyError> {
        Err(CovyError::Parse {
            format: "lcov".to_string(),
            detail: "invalid fixture".to_string(),
        })
    }

    fn adapters() -> PipelineIngestAdapters {
        PipelineIngestAdapters {
            ingest_coverage_auto: ingest_cov_auto_stub,
            ingest_coverage_with_format: ingest_cov_with_format_stub,
            ingest_coverage_stdin: ingest_cov_stdin_stub,
            ingest_diagnostics: ingest_diag_stub,
        }
    }

    fn adapters_with_coverage_failure() -> PipelineIngestAdapters {
        PipelineIngestAdapters {
            ingest_coverage_auto: ingest_cov_auto_failure,
            ..adapters()
        }
    }

    fn make_coverage(path: &str) -> CoverageData {
        let mut coverage = CoverageData::new();
        let mut fc = FileCoverage::new();
        fc.lines_instrumented.insert(1);
        fc.lines_covered.insert(1);
        coverage.files.insert(path.to_string(), fc);
        coverage
    }

    fn make_issue(path: &str, line: u32, fingerprint: &str) -> Issue {
        Issue {
            path: path.to_string(),
            line,
            column: None,
            end_line: None,
            severity: Severity::Error,
            rule_id: "R".to_string(),
            message: "m".to_string(),
            source: "s".to_string(),
            fingerprint: fingerprint.to_string(),
        }
    }

    #[test]
    fn test_resolve_coverage_paths_take_precedence_when_allowed() {
        let temp = tempfile::tempdir().unwrap();
        let report = temp.path().join("a.info");
        std::fs::write(&report, "ignored").unwrap();

        let state_path = temp.path().join("state.bin");
        let state_cov = make_coverage("src/from-state.rs");
        let state_bytes = suite_foundation_core::cache::serialize_coverage(&state_cov).unwrap();
        std::fs::write(&state_path, state_bytes).unwrap();

        let input = PipelineCoverageInput {
            paths: vec![report.display().to_string()],
            format: None,
            stdin: false,
            input_state_path: Some(state_path.display().to_string()),
            default_input_state_path: None,
            strip_prefixes: Vec::new(),
            reject_paths_with_input: false,
            no_inputs_error: "missing".to_string(),
        };

        let coverage = resolve_coverage_input(&input, &adapters()).unwrap();
        assert!(coverage.files.contains_key("src/from-paths.rs"));
        assert!(!coverage.files.contains_key("src/from-state.rs"));
    }

    #[test]
    fn resolve_coverage_rejects_paths_with_input_using_typed_variant() {
        let input = PipelineCoverageInput {
            paths: vec!["*.info".to_string()],
            format: None,
            stdin: false,
            input_state_path: Some(".covy/state/latest.bin".to_string()),
            default_input_state_path: Some(".covy/state/latest.bin".to_string()),
            strip_prefixes: Vec::new(),
            reject_paths_with_input: true,
            no_inputs_error: "missing".to_string(),
        };

        let error = resolve_coverage_input(&input, &adapters()).unwrap_err();

        assert!(matches!(
            error,
            DiffyError::ConflictingInputs {
                first: "positional coverage paths",
                second: "--input"
            }
        ));
    }

    #[test]
    fn resolve_coverage_missing_default_state_has_stable_display() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing.bin");

        let input = PipelineCoverageInput {
            paths: Vec::new(),
            format: None,
            stdin: false,
            input_state_path: None,
            default_input_state_path: Some(missing.display().to_string()),
            strip_prefixes: Vec::new(),
            reject_paths_with_input: true,
            no_inputs_error: "missing".to_string(),
        };

        let error = resolve_coverage_input(&input, &adapters()).unwrap_err();

        assert_eq!(
            error.to_string(),
            format!(
                "No coverage files specified and no cached coverage state found at {}. Provide file paths, use --stdin, or run `covy ingest` first.",
                missing.display()
            )
        );
    }

    #[test]
    fn resolve_coverage_stdin_without_format_uses_typed_variant() {
        let input = PipelineCoverageInput {
            paths: Vec::new(),
            format: None,
            stdin: true,
            input_state_path: None,
            default_input_state_path: None,
            strip_prefixes: Vec::new(),
            reject_paths_with_input: true,
            no_inputs_error: "missing".to_string(),
        };

        let error = resolve_coverage_input(&input, &adapters()).unwrap_err();

        assert!(matches!(error, DiffyError::MissingStdinFormat));
    }

    #[test]
    fn resolve_coverage_invalid_glob_preserves_pattern_source() {
        let error =
            resolve_globs(&["[".to_string()], "coverage").expect_err("invalid glob must fail");

        let source = error.source().expect("invalid glob must have a source");

        assert!(source.downcast_ref::<glob::PatternError>().is_some());
    }

    #[test]
    fn resolve_coverage_ingest_failure_preserves_typed_source() {
        let temp = tempfile::tempdir().unwrap();
        let report = temp.path().join("invalid.info");
        std::fs::write(&report, "invalid").unwrap();
        let input = PipelineCoverageInput {
            paths: vec![report.display().to_string()],
            format: None,
            stdin: false,
            input_state_path: None,
            default_input_state_path: None,
            strip_prefixes: Vec::new(),
            reject_paths_with_input: true,
            no_inputs_error: "missing".to_string(),
        };

        let error = resolve_coverage_input(&input, &adapters_with_coverage_failure()).unwrap_err();
        let source = error.source().expect("ingestion error must have a source");

        assert!(matches!(
            source.downcast_ref::<CovyError>(),
            Some(CovyError::Parse { format, detail })
                if format == "lcov" && detail == "invalid fixture"
        ));
    }

    #[test]
    fn load_coverage_state_corruption_uses_decode_variant() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("invalid.bin");
        std::fs::write(&state, [0_u8]).unwrap();

        let error = load_coverage_state(&state).unwrap_err();

        assert!(matches!(
            error,
            DiffyError::CoverageStateDecode {
                source: CovyError::Cache(_),
                ..
            }
        ));
    }

    #[test]
    fn test_state_metadata_compatibility_checks_versions_and_root() {
        let mut meta = DiagnosticsStateMetadata::normalized_for_repo_root(
            suite_foundation_core::cache::current_repo_root_id(None),
        );
        assert!(state_metadata_compatible(Some(&meta), None));

        meta.path_norm_version += 1;
        assert!(!state_metadata_compatible(Some(&meta), None));

        let unversioned = DiagnosticsStateMetadata::unversioned();
        assert!(!state_metadata_compatible(Some(&unversioned), None));
    }

    #[test]
    fn test_resolve_diagnostics_state_selective_by_changed_paths() {
        let temp = tempfile::tempdir().unwrap();
        let state_path = temp.path().join("issues.bin");

        let mut diagnostics = DiagnosticsData::new();
        diagnostics.issues_by_file.insert(
            "src/main.rs".to_string(),
            vec![make_issue("src/main.rs", 10, "fp-main")],
        );
        diagnostics.issues_by_file.insert(
            "src/lib.rs".to_string(),
            vec![make_issue("src/lib.rs", 20, "fp-lib")],
        );

        let metadata = DiagnosticsStateMetadata::normalized_for_repo_root(
            suite_foundation_core::cache::current_repo_root_id(None),
        );
        let bytes = suite_foundation_core::cache::serialize_diagnostics_with_metadata(
            &diagnostics,
            &metadata,
        )
        .unwrap();
        std::fs::write(&state_path, bytes).unwrap();

        let input = PipelineDiagnosticsInput {
            issue_patterns: Vec::new(),
            issues_state_path: Some(state_path.display().to_string()),
            no_issues_state: false,
            default_issues_state_path: ".covy/state/issues.bin".to_string(),
        };
        let selected_paths: HashSet<String> = ["src/main.rs".to_string()].into_iter().collect();

        let loaded = resolve_diagnostics_input(&input, &selected_paths, None, &adapters()).unwrap();
        let loaded = loaded.data.expect("diagnostics should load");
        assert_eq!(loaded.issues_by_file.len(), 1);
        assert!(loaded.issues_by_file.contains_key("src/main.rs"));
    }
}
