use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{AdapterResult, Result, TestyError};

/// Error returned by test-map orchestration.
pub type TestMapError = TestyError;

pub const TESTMAP_MANIFEST_SCHEMA_EXAMPLE: &str = r#"{
  "type": "testmap-build-manifest-jsonl",
  "description": "One JSON object per line.",
  "example_line": {
    "test_id": "com.foo.BarTest",
    "language": "java",
    "duration_ms": 1200,
    "coverage_report": "path/to/jacoco.xml",
    "coverage_reports": ["path/to/jacoco.xml", "path/to/extra.xml"]
  }
}"#;

#[derive(Debug, Clone)]
pub struct TestMapRequest {
    pub manifest_globs: Vec<String>,
    pub output_testmap_path: String,
    pub output_timings_path: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TestMapStats {
    pub manifest_files: usize,
    pub records: usize,
    pub tests: usize,
    pub files: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TestMapRecord {
    pub test_id: String,
    pub language: String,
    pub duration_ms: Option<u64>,
    pub coverage_reports: Vec<String>,
    pub covered_files: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TestMapResponse {
    pub map_records: Vec<TestMapRecord>,
    pub stats: TestMapStats,
    pub warnings: Vec<String>,
    pub output_testmap_path: String,
    pub output_timings_path: String,
}

#[derive(Clone, Copy)]
pub struct TestMapAdapters {
    pub ingest_coverage: fn(&Path) -> AdapterResult<crate::model::CoverageData>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct TestMapManifestRecord {
    pub test_id: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub coverage_report: Option<String>,
    #[serde(default)]
    pub coverage_reports: Vec<String>,
}

impl TestMapManifestRecord {
    pub fn coverage_paths(&self) -> Vec<&str> {
        let mut paths = Vec::new();
        if let Some(path) = self.coverage_report.as_deref() {
            paths.push(path);
        }
        for path in &self.coverage_reports {
            paths.push(path.as_str());
        }
        paths
    }
}

/// Resolve manifests, build test-map state, and write its persisted outputs.
///
/// # Errors
///
/// Returns [`TestMapError`] when glob resolution, manifest decoding, coverage
/// ingestion, validation, state encoding, or output I/O fails.
pub fn run_testmap(req: TestMapRequest, adapters: &TestMapAdapters) -> Result<TestMapResponse> {
    let TestMapRequest {
        manifest_globs,
        output_testmap_path,
        output_timings_path,
    } = req;

    let (files, warnings) = resolve_manifest_globs(&manifest_globs)?;
    if files.is_empty() {
        return Err(TestyError::invalid("No manifest files found"));
    }

    let records = load_manifest_records(&files)?;
    validate_manifest_records(&records)?;
    let artifacts = build_testmap_artifacts(&records, adapters)?;

    write_testmap(Path::new(&output_testmap_path), &artifacts.index)?;
    write_test_timing_history(Path::new(&output_timings_path), &artifacts.timings)?;

    Ok(TestMapResponse {
        map_records: artifacts.map_records,
        stats: TestMapStats {
            manifest_files: files.len(),
            records: records.len(),
            tests: artifacts.index.test_to_files.len(),
            files: artifacts.index.file_to_tests.len(),
        },
        warnings,
        output_testmap_path,
        output_timings_path,
    })
}

/// Resolve manifest globs and return unmatched-pattern warnings.
///
/// # Errors
///
/// Returns [`TestyError::GlobPattern`] when a pattern is invalid.
pub fn resolve_manifest_globs(patterns: &[String]) -> Result<(Vec<PathBuf>, Vec<String>)> {
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    for pattern in patterns {
        let matches: Vec<_> = glob::glob(pattern)
            .map_err(|source| TestyError::GlobPattern {
                pattern: pattern.clone(),
                source,
            })?
            .filter_map(|r| r.ok())
            .collect();
        if matches.is_empty() {
            warnings.push(format!("No files matched pattern: {pattern}"));
        }
        files.extend(matches);
    }
    Ok((files, warnings))
}

/// Read and decode JSONL test-map manifest records.
///
/// # Errors
///
/// Returns [`TestyError::Io`] when a manifest cannot be read or
/// [`TestyError::Json`] when a non-empty line is invalid JSON.
pub fn load_manifest_records(files: &[PathBuf]) -> Result<Vec<TestMapManifestRecord>> {
    let mut out = Vec::new();
    for file in files {
        let content = std::fs::read_to_string(file)
            .map_err(|source| TestyError::io("Failed to read manifest file", file, source))?;
        for (idx, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let rec: TestMapManifestRecord = serde_json::from_str(line).map_err(|source| {
                TestyError::Json {
                    context: format!("Invalid JSON on {} line {}", file.display(), idx + 1),
                    source,
                    example: "\n\nExpected JSONL shape (one per line):\n  {\"test_id\": \"com.foo.BarTest\", \"language\": \"java\", \"duration_ms\": 1200, \"coverage_report\": \"path/to/report.xml\"}".to_string(),
                }
            })?;
            out.push(rec);
        }
    }
    Ok(out)
}

/// Validate semantic invariants of decoded manifest records.
///
/// # Errors
///
/// Returns [`TestyError::InvalidInput`] for an empty manifest, invalid test
/// identifier or language, or a record without coverage input.
pub fn validate_manifest_records(records: &[TestMapManifestRecord]) -> Result<()> {
    if records.is_empty() {
        return Err(TestyError::invalid("Manifest contains no records"));
    }
    for (idx, rec) in records.iter().enumerate() {
        if rec.test_id.trim().is_empty() {
            return Err(TestyError::invalid(format!(
                "Record {} has empty test_id",
                idx + 1
            )));
        }
        if let Some(language) = rec.language.as_deref() {
            if language.trim().is_empty() {
                return Err(TestyError::invalid(format!(
                    "Record {} has empty language",
                    idx + 1
                )));
            }
            if normalize_language(language).is_none() {
                return Err(TestyError::invalid(format!(
                    "Record {} has unsupported language '{}' (expected java or python)",
                    idx + 1,
                    language
                )));
            }
        }
        if rec.coverage_report.as_deref().is_none() && rec.coverage_reports.is_empty() {
            return Err(TestyError::invalid(format!(
                "Record {} for test '{}' must provide coverage_report or coverage_reports",
                idx + 1,
                rec.test_id
            )));
        }
    }
    Ok(())
}

pub struct TestMapBuildArtifacts {
    pub index: crate::testmap::TestMapIndex,
    pub timings: crate::testmap::TestTimingHistory,
    pub map_records: Vec<TestMapRecord>,
}

/// Build in-memory test-map and timing artifacts from validated records.
///
/// # Errors
///
/// Returns [`TestyError::Adapter`] when an injected coverage adapter fails,
/// or [`TestyError::InvalidInput`] for an unsupported language.
pub fn build_testmap_artifacts(
    records: &[TestMapManifestRecord],
    adapters: &TestMapAdapters,
) -> Result<TestMapBuildArtifacts> {
    let mut index = crate::testmap::TestMapIndex::default();
    index.metadata.schema_version = crate::cache::TESTMAP_SCHEMA_VERSION;
    index.metadata.path_norm_version = crate::cache::DIAGNOSTICS_PATH_NORM_VERSION;
    index.metadata.repo_root_id = crate::cache::current_repo_root_id(None);
    index.metadata.generated_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    index.metadata.granularity = "file".to_string();

    let mut test_to_files: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut map_records: BTreeMap<String, TestMapRecord> = BTreeMap::new();
    let mut coverage_cache: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for rec in records {
        let mut covered_files = BTreeSet::new();
        for coverage_path in rec.coverage_paths() {
            let normalized_files = if let Some(files) = coverage_cache.get(coverage_path) {
                files.clone()
            } else {
                let mut coverage =
                    (adapters.ingest_coverage)(Path::new(coverage_path)).map_err(|source| {
                        TestyError::adapter(
                            format!(
                                "Failed to ingest coverage report '{}' for test '{}'",
                                coverage_path, rec.test_id
                            ),
                            source,
                        )
                    })?;
                suite_foundation_core::pathmap::auto_normalize_paths(&mut coverage, None);
                let files = coverage.files.keys().cloned().collect::<BTreeSet<_>>();
                coverage_cache.insert(coverage_path.to_string(), files.clone());
                files
            };

            covered_files.extend(normalized_files);
        }

        let language = resolve_language(rec)?;
        index
            .test_language
            .insert(rec.test_id.clone(), language.clone());
        test_to_files.insert(rec.test_id.clone(), covered_files.clone());

        map_records.insert(
            rec.test_id.clone(),
            TestMapRecord {
                test_id: rec.test_id.clone(),
                language,
                duration_ms: rec.duration_ms,
                coverage_reports: rec
                    .coverage_paths()
                    .into_iter()
                    .map(ToString::to_string)
                    .collect(),
                covered_files: covered_files.into_iter().collect(),
            },
        );
    }

    index.test_to_files = test_to_files;
    index.file_to_tests = build_file_to_tests_index(&index.test_to_files);

    let timings = build_test_timing_history(records);

    Ok(TestMapBuildArtifacts {
        index,
        timings,
        map_records: map_records.into_values().collect(),
    })
}

pub fn build_file_to_tests_index(
    test_to_files: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut file_to_tests: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (test_id, files) in test_to_files {
        for file in files {
            file_to_tests
                .entry(file.clone())
                .or_default()
                .insert(test_id.clone());
        }
    }
    file_to_tests
}

pub fn build_test_timing_history(
    records: &[TestMapManifestRecord],
) -> crate::testmap::TestTimingHistory {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut history = crate::testmap::TestTimingHistory {
        generated_at: now,
        ..Default::default()
    };
    for rec in records {
        if let Some(duration_ms) = rec.duration_ms {
            history.duration_ms.insert(rec.test_id.clone(), duration_ms);
            history.sample_count.insert(rec.test_id.clone(), 1);
            history.last_seen.insert(rec.test_id.clone(), now);
        }
    }
    history
}

/// Write a test map using the versioned workspace state codec.
///
/// # Errors
///
/// Returns [`TestyError::Io`] for directory or file failures and
/// [`TestyError::State`] when encoding fails.
pub fn write_testmap(path: &Path, index: &crate::testmap::TestMapIndex) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| {
            TestyError::io("Failed to create testmap parent directory", parent, source)
        })?;
    }
    let bytes = crate::cache::serialize_testmap(index)
        .map_err(|source| TestyError::state("Failed to encode testmap at", path, source))?;
    std::fs::write(path, bytes)
        .map_err(|source| TestyError::io("Failed to write testmap at", path, source))?;
    Ok(())
}

/// Write test timing history using the versioned workspace state codec.
///
/// # Errors
///
/// Returns [`TestyError::Io`] for directory or file failures and
/// [`TestyError::State`] when encoding fails.
pub fn write_test_timing_history(
    path: &Path,
    timings: &crate::testmap::TestTimingHistory,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| {
            TestyError::io(
                "Failed to create test timing parent directory",
                parent,
                source,
            )
        })?;
    }
    let bytes = crate::cache::serialize_test_timings(timings)
        .map_err(|source| TestyError::state("Failed to encode test timings at", path, source))?;
    std::fs::write(path, bytes)
        .map_err(|source| TestyError::io("Failed to write test timings at", path, source))?;
    Ok(())
}

/// Load and decode a persisted test map.
///
/// # Errors
///
/// Returns [`TestyError::Io`] when the state cannot be read or
/// [`TestyError::State`] when its versioned payload is invalid.
pub fn load_testmap(path: &Path) -> Result<crate::testmap::TestMapIndex> {
    let bytes = std::fs::read(path)
        .map_err(|source| TestyError::io("Failed to read testmap at", path, source))?;
    crate::cache::deserialize_testmap(&bytes)
        .map_err(|source| TestyError::state("Failed to decode testmap at", path, source))
}

/// Load and decode persisted test timing history.
///
/// # Errors
///
/// Returns [`TestyError::Io`] when the state cannot be read or
/// [`TestyError::State`] when its versioned payload is invalid.
pub fn load_test_timing_history(path: &Path) -> Result<crate::testmap::TestTimingHistory> {
    let bytes = std::fs::read(path)
        .map_err(|source| TestyError::io("Failed to read test timings at", path, source))?;
    crate::cache::deserialize_test_timings(&bytes)
        .map_err(|source| TestyError::state("Failed to decode test timings at", path, source))
}

fn resolve_language(rec: &TestMapManifestRecord) -> Result<String> {
    if let Some(raw) = rec.language.as_deref() {
        return normalize_language(raw).ok_or_else(|| {
            TestyError::invalid(format!(
                "Unsupported language '{}' for {}",
                raw, rec.test_id
            ))
        });
    }
    Ok(infer_language_from_test_id(&rec.test_id))
}

fn normalize_language(raw: &str) -> Option<String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "java" => Some("java".to_string()),
        "python" | "py" => Some("python".to_string()),
        _ => None,
    }
}

fn infer_language_from_test_id(test_id: &str) -> String {
    if test_id.contains("::") {
        "python".to_string()
    } else {
        "java".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn fake_ingest(path: &Path) -> AdapterResult<crate::model::CoverageData> {
        let mut coverage = crate::model::CoverageData::new();
        let mut fc = crate::model::FileCoverage::new();
        fc.lines_instrumented.insert(1);
        fc.lines_covered.insert(1);
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        coverage.files.insert(format!("src/{name}.rs"), fc);
        Ok(coverage)
    }

    fn adapters() -> TestMapAdapters {
        TestMapAdapters {
            ingest_coverage: fake_ingest,
        }
    }

    static INGEST_CALLS: AtomicUsize = AtomicUsize::new(0);

    fn counting_ingest(path: &Path) -> AdapterResult<crate::model::CoverageData> {
        INGEST_CALLS.fetch_add(1, Ordering::SeqCst);
        fake_ingest(path)
    }

    #[test]
    fn test_validate_manifest_records_success() {
        let records = vec![TestMapManifestRecord {
            test_id: "com.foo.BarTest".to_string(),
            language: Some("java".to_string()),
            duration_ms: Some(123),
            coverage_report: Some("reports/bar.xml".to_string()),
            coverage_reports: Vec::new(),
        }];
        assert!(validate_manifest_records(&records).is_ok());
    }

    #[test]
    fn test_validate_manifest_records_missing_coverage() {
        let records = vec![TestMapManifestRecord {
            test_id: "com.foo.BarTest".to_string(),
            language: None,
            duration_ms: None,
            coverage_report: None,
            coverage_reports: Vec::new(),
        }];
        let err = validate_manifest_records(&records).unwrap_err();
        assert!(err
            .to_string()
            .contains("must provide coverage_report or coverage_reports"));
    }

    #[test]
    fn test_manifest_record_coverage_paths() {
        let rec = TestMapManifestRecord {
            test_id: "t".to_string(),
            language: None,
            duration_ms: None,
            coverage_report: Some("a.info".to_string()),
            coverage_reports: vec!["b.info".to_string()],
        };
        assert_eq!(rec.coverage_paths(), vec!["a.info", "b.info"]);
    }

    #[test]
    fn test_build_file_to_tests_index() {
        let mut test_to_files: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        test_to_files
            .entry("t1".to_string())
            .or_default()
            .insert("src/a.rs".to_string());
        test_to_files
            .entry("t2".to_string())
            .or_default()
            .insert("src/a.rs".to_string());
        test_to_files
            .entry("t2".to_string())
            .or_default()
            .insert("src/b.rs".to_string());

        let idx = build_file_to_tests_index(&test_to_files);
        assert_eq!(idx["src/a.rs"].len(), 2);
        assert_eq!(idx["src/b.rs"].len(), 1);
    }

    #[test]
    fn test_build_artifacts_infers_python_nodeid() {
        let records = vec![TestMapManifestRecord {
            test_id: "tests/test_a.py::test_x".to_string(),
            language: None,
            duration_ms: None,
            coverage_report: Some("a.info".to_string()),
            coverage_reports: Vec::new(),
        }];
        let artifacts = build_testmap_artifacts(&records, &adapters()).unwrap();
        assert_eq!(
            artifacts.index.test_language["tests/test_a.py::test_x"],
            "python"
        );
    }

    #[test]
    fn test_build_artifacts_reuses_cached_coverage_report_ingest() {
        INGEST_CALLS.store(0, Ordering::SeqCst);
        let records = vec![
            TestMapManifestRecord {
                test_id: "com.foo.FirstTest".to_string(),
                language: Some("java".to_string()),
                duration_ms: Some(10),
                coverage_report: Some("reports/shared.info".to_string()),
                coverage_reports: Vec::new(),
            },
            TestMapManifestRecord {
                test_id: "com.foo.SecondTest".to_string(),
                language: Some("java".to_string()),
                duration_ms: Some(20),
                coverage_report: Some("reports/shared.info".to_string()),
                coverage_reports: Vec::new(),
            },
        ];

        let artifacts = build_testmap_artifacts(
            &records,
            &TestMapAdapters {
                ingest_coverage: counting_ingest,
            },
        )
        .unwrap();

        assert_eq!(artifacts.index.test_to_files.len(), 2);
        assert_eq!(INGEST_CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_build_test_timing_history_from_manifest_durations() {
        let records = vec![
            TestMapManifestRecord {
                test_id: "com.foo.BarTest".to_string(),
                language: Some("java".to_string()),
                duration_ms: Some(1200),
                coverage_report: Some("a.info".to_string()),
                coverage_reports: Vec::new(),
            },
            TestMapManifestRecord {
                test_id: "tests/test_mod.py::test_one".to_string(),
                language: Some("python".to_string()),
                duration_ms: Some(900),
                coverage_report: Some("b.info".to_string()),
                coverage_reports: Vec::new(),
            },
        ];
        let timings = build_test_timing_history(&records);
        assert_eq!(timings.duration_ms.get("com.foo.BarTest"), Some(&1200));
        assert_eq!(
            timings.duration_ms.get("tests/test_mod.py::test_one"),
            Some(&900)
        );
        assert_eq!(timings.sample_count.get("com.foo.BarTest"), Some(&1));
        assert!(timings.generated_at > 0);
    }

    #[test]
    fn test_run_testmap_builds_outputs_and_summary() {
        let dir = tempfile::TempDir::new().unwrap();
        let manifest = dir.path().join("manifest.jsonl");
        let output = dir.path().join("state").join("testmap.bin");
        let timings = dir.path().join("state").join("testtimings.bin");

        std::fs::write(
            &manifest,
            "{\"test_id\":\"com.foo.BarTest\",\"language\":\"java\",\"duration_ms\":123,\"coverage_report\":\"reports/a.info\"}\n",
        )
        .unwrap();

        let resp = run_testmap(
            TestMapRequest {
                manifest_globs: vec![manifest.to_string_lossy().to_string()],
                output_testmap_path: output.to_string_lossy().to_string(),
                output_timings_path: timings.to_string_lossy().to_string(),
            },
            &adapters(),
        )
        .unwrap();

        assert_eq!(resp.stats.manifest_files, 1);
        assert_eq!(resp.stats.records, 1);
        assert_eq!(resp.stats.tests, 1);
        assert!(resp.warnings.is_empty());
        assert!(output.exists());
        assert!(timings.exists());
    }
}
