use serde::Deserialize;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Failure to locate, read, or parse a Covy configuration file.
///
/// A missing configuration file is not an error: [`CovyConfig::load`] and
/// [`CovyConfig::find_and_load`] return the default configuration when every
/// candidate is absent. All other filesystem and parse failures are reported
/// with the path that caused them.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigLoadError {
    #[error("failed to determine the current directory while searching for covy.toml: {source}")]
    CurrentDirectory {
        #[source]
        source: io::Error,
    },

    #[error("failed to read config at '{}': {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to parse config at '{}': {source}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct CovyConfig {
    pub project: ProjectConfig,
    pub ingest: IngestConfig,
    pub diff: DiffConfig,
    pub gate: GateConfig,
    pub report: ReportConfig,
    pub cache: CacheConfig,
    pub impact: ImpactConfig,
    pub shard: ShardConfig,
    pub merge: MergeConfig,
    pub paths: PathsConfig,
    #[serde(alias = "path_mapping")]
    pub path_mapping: PathMappingConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    pub name: String,
    pub source_root: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct IngestConfig {
    pub report_paths: Vec<String>,
    pub strip_prefixes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DiffConfig {
    pub base: String,
    pub head: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct GateConfig {
    pub fail_under_total: Option<f64>,
    pub fail_under_changed: Option<f64>,
    pub fail_under_new: Option<f64>,
    #[serde(default)]
    pub issues: IssueGateConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct IssueGateConfig {
    pub max_new_errors: Option<u32>,
    pub max_new_warnings: Option<u32>,
    pub max_new_issues: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ReportConfig {
    pub format: String,
    pub show_missing: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    pub enabled: bool,
    pub dir: String,
    pub max_age_days: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ImpactConfig {
    pub testmap_path: String,
    pub max_tests: usize,
    pub target_coverage: f64,
    pub stale_after_days: u32,
    pub allow_stale: bool,
    pub test_id_strategy: String,
    // Legacy fields kept for backward compatibility.
    pub fresh_hours: u32,
    pub full_suite_threshold: f64,
    pub fallback_mode: String,
    pub smoke: ImpactSmokeConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ImpactSmokeConfig {
    pub always: Vec<String>,
    pub stale_extra: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ShardConfig {
    pub timings_path: String,
    pub algorithm: String,
    pub unknown_test_seconds: f64,
    pub tiers: ShardTiersConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ShardTiersConfig {
    pub pr: ShardTierConfig,
    pub nightly: ShardTierConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ShardTierConfig {
    pub exclude_tags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MergeConfig {
    pub strict: bool,
    pub output_coverage: String,
    pub output_issues: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct PathMappingConfig {
    pub rules: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PathsConfig {
    pub strip_prefix: Vec<String>,
    pub replace_prefix: Vec<ReplacePrefixRule>,
    pub ignore_globs: Vec<String>,
    pub case_sensitive: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ReplacePrefixRule {
    pub from: String,
    pub to: String,
}

// Defaults

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            source_root: ".".to_string(),
        }
    }
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self {
            base: "main".to_string(),
            head: "HEAD".to_string(),
        }
    }
}

impl Default for ReportConfig {
    fn default() -> Self {
        Self {
            format: "terminal".to_string(),
            show_missing: false,
        }
    }
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            dir: ".covy/cache".to_string(),
            max_age_days: 30,
        }
    }
}

impl Default for ImpactConfig {
    fn default() -> Self {
        Self {
            testmap_path: ".covy/state/testmap.bin".to_string(),
            max_tests: 25,
            target_coverage: 0.90,
            stale_after_days: 14,
            allow_stale: true,
            test_id_strategy: "junit".to_string(),
            fresh_hours: 24,
            full_suite_threshold: 0.40,
            fallback_mode: "fail-open".to_string(),
            smoke: ImpactSmokeConfig::default(),
        }
    }
}

impl Default for ShardConfig {
    fn default() -> Self {
        Self {
            timings_path: ".covy/state/testtimings.bin".to_string(),
            algorithm: "lpt".to_string(),
            unknown_test_seconds: 8.0,
            tiers: ShardTiersConfig::default(),
        }
    }
}

impl Default for ShardTiersConfig {
    fn default() -> Self {
        Self {
            pr: ShardTierConfig {
                exclude_tags: vec!["slow".to_string()],
            },
            nightly: ShardTierConfig::default(),
        }
    }
}

impl Default for MergeConfig {
    fn default() -> Self {
        Self {
            strict: true,
            output_coverage: ".covy/state/latest.bin".to_string(),
            output_issues: ".covy/state/issues.bin".to_string(),
        }
    }
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            strip_prefix: Vec::new(),
            replace_prefix: Vec::new(),
            ignore_globs: vec![
                "**/target/**".to_string(),
                "**/node_modules/**".to_string(),
                "**/bazel-out/**".to_string(),
            ],
            case_sensitive: !cfg!(windows),
        }
    }
}

impl CovyConfig {
    /// Load config from a TOML file.
    ///
    /// Returns the default configuration only when `path` does not exist.
    /// Unreadable files, directories, and malformed TOML are errors.
    pub fn load(path: &Path) -> Result<Self, ConfigLoadError> {
        Ok(Self::load_if_present(path)?.unwrap_or_default())
    }

    /// Search for covy.toml in the current directory and parents.
    ///
    /// Returns the default configuration only when no candidate exists.
    /// Filesystem and parse failures from an encountered candidate are
    /// propagated rather than treated as an absent configuration.
    pub fn find_and_load() -> Result<Self, ConfigLoadError> {
        let dir = std::env::current_dir()
            .map_err(|source| ConfigLoadError::CurrentDirectory { source })?;
        Self::find_and_load_from(&dir)
    }

    fn find_and_load_from(start_dir: &Path) -> Result<Self, ConfigLoadError> {
        let mut dir = start_dir.to_path_buf();
        loop {
            let candidate = dir.join("covy.toml");
            if let Some(config) = Self::load_if_present(&candidate)? {
                return Ok(config);
            }
            if !dir.pop() {
                break;
            }
        }
        Ok(Self::default())
    }

    fn load_if_present(path: &Path) -> Result<Option<Self>, ConfigLoadError> {
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ConfigLoadError::Read {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };

        toml::from_str(&content)
            .map(Some)
            .map_err(|source| ConfigLoadError::Parse {
                path: path.to_path_buf(),
                source,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn load_defaults_only_when_config_is_missing() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("missing.toml");

        let config = CovyConfig::load(&missing).unwrap();

        assert_eq!(config.diff.base, "main");
        assert_eq!(config.diff.head, "HEAD");
    }

    #[test]
    fn load_reads_valid_config() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("covy.toml");
        std::fs::write(
            &path,
            r#"
                [project]
                name = "demo"

                [gate]
                fail_under_total = 91.5
            "#,
        )
        .unwrap();

        let config = CovyConfig::load(&path).unwrap();

        assert_eq!(config.project.name, "demo");
        assert_eq!(config.gate.fail_under_total, Some(91.5));
    }

    #[test]
    fn load_reports_malformed_config_with_its_path() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("covy.toml");
        std::fs::write(&path, "[gate\nfail_under_total = 90").unwrap();

        let error = CovyConfig::load(&path).unwrap_err();

        assert!(matches!(
            error,
            ConfigLoadError::Parse {
                path: error_path,
                ..
            } if error_path == path
        ));
    }

    #[test]
    fn load_reports_directory_instead_of_defaulting() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("covy.toml");
        std::fs::create_dir(&path).unwrap();

        let error = CovyConfig::load(&path).unwrap_err();

        assert!(matches!(
            error,
            ConfigLoadError::Read {
                path: error_path,
                ..
            } if error_path == path
        ));
    }

    #[cfg(unix)]
    #[test]
    fn load_reports_unreadable_config_when_permissions_are_enforced() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let path = dir.path().join("covy.toml");
        std::fs::write(&path, "[project]\nname = \"demo\"\n").unwrap();
        let original_permissions = std::fs::metadata(&path).unwrap().permissions();
        let mut unreadable_permissions = original_permissions.clone();
        unreadable_permissions.set_mode(0o000);
        std::fs::set_permissions(&path, unreadable_permissions).unwrap();

        let permissions_are_enforced = std::fs::read_to_string(&path).is_err();
        let result = CovyConfig::load(&path);
        std::fs::set_permissions(&path, original_permissions).unwrap();

        if permissions_are_enforced {
            assert!(matches!(
                result.unwrap_err(),
                ConfigLoadError::Read {
                    path: error_path,
                    source,
                } if error_path == path && source.kind() == io::ErrorKind::PermissionDenied
            ));
        }
    }

    #[test]
    fn find_and_load_searches_parents_without_masking_parse_errors() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("one").join("two");
        std::fs::create_dir_all(&nested).unwrap();
        let path = dir.path().join("covy.toml");
        std::fs::write(&path, "[project]\nname = \"found\"\n").unwrap();

        let config = CovyConfig::find_and_load_from(&nested).unwrap();
        assert_eq!(config.project.name, "found");

        std::fs::write(&path, "[project\nname = \"broken\"").unwrap();
        let error = CovyConfig::find_and_load_from(&nested).unwrap_err();
        assert!(matches!(
            error,
            ConfigLoadError::Parse {
                path: error_path,
                ..
            } if error_path == path
        ));
    }

    #[test]
    fn find_and_load_propagates_candidate_read_errors() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("one");
        std::fs::create_dir_all(nested.join("covy.toml")).unwrap();

        let error = CovyConfig::find_and_load_from(&nested).unwrap_err();

        assert!(matches!(
            error,
            ConfigLoadError::Read {
                path: error_path,
                ..
            } if error_path == nested.join("covy.toml")
        ));
    }

    #[test]
    fn production_config_loads_never_erase_errors() {
        fn visit_rust_sources(dir: &Path, failures: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    visit_rust_sources(&path, failures);
                } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                    let source = std::fs::read_to_string(&path).unwrap();
                    let compact: String = source.split_whitespace().collect();
                    let load_call = ["CovyConfig", "::", "load", "("].concat();
                    let erased_error = [".unwrap_or_", "default()"].concat();
                    let mut remainder = compact.as_str();
                    while let Some(index) = remainder.find(&load_call) {
                        let statement = &remainder[index..];
                        let statement = statement
                            .split_once(';')
                            .map_or(statement, |(statement, _)| statement);
                        if statement.contains(&erased_error) {
                            failures.push(path.clone());
                            break;
                        }
                        remainder = &remainder[index + load_call.len()..];
                    }
                }
            }
        }

        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap();
        let mut failures = Vec::new();
        visit_rust_sources(&workspace.join("crates"), &mut failures);

        assert!(
            failures.is_empty(),
            "production config loads must propagate errors; violations: {failures:?}"
        );
    }

    #[test]
    fn test_deserialize_gate_issues_defaults() {
        let raw = r#"
            [gate]
            fail_under_total = 80.0
        "#;
        let config: CovyConfig = toml::from_str(raw).unwrap();
        assert_eq!(config.gate.fail_under_total, Some(80.0));
        assert_eq!(config.gate.issues.max_new_errors, None);
        assert_eq!(config.gate.issues.max_new_warnings, None);
        assert_eq!(config.gate.issues.max_new_issues, None);
    }

    #[test]
    fn test_deserialize_gate_issues_configured() {
        let raw = r#"
            [gate]
            fail_under_changed = 90.0

            [gate.issues]
            max_new_errors = 0
            max_new_warnings = 5
            max_new_issues = 8
        "#;
        let config: CovyConfig = toml::from_str(raw).unwrap();
        assert_eq!(config.gate.fail_under_changed, Some(90.0));
        assert_eq!(config.gate.issues.max_new_errors, Some(0));
        assert_eq!(config.gate.issues.max_new_warnings, Some(5));
        assert_eq!(config.gate.issues.max_new_issues, Some(8));
    }

    #[test]
    fn test_deserialize_impact_shard_merge_defaults() {
        let raw = r#"
            [project]
            name = "demo"
        "#;
        let config: CovyConfig = toml::from_str(raw).unwrap();
        assert_eq!(config.impact.testmap_path, ".covy/state/testmap.bin");
        assert_eq!(config.impact.max_tests, 25);
        assert!((config.impact.target_coverage - 0.90).abs() < f64::EPSILON);
        assert_eq!(config.impact.stale_after_days, 14);
        assert!(config.impact.allow_stale);
        assert_eq!(config.impact.test_id_strategy, "junit");
        assert_eq!(config.impact.fresh_hours, 24);
        assert!((config.impact.full_suite_threshold - 0.40).abs() < f64::EPSILON);
        assert_eq!(config.impact.fallback_mode, "fail-open");
        assert_eq!(config.shard.algorithm, "lpt");
        assert!((config.shard.unknown_test_seconds - 8.0).abs() < f64::EPSILON);
        assert_eq!(config.shard.tiers.pr.exclude_tags, vec!["slow".to_string()]);
        assert!(config.shard.tiers.nightly.exclude_tags.is_empty());
        assert!(config.merge.strict);
        assert_eq!(config.merge.output_coverage, ".covy/state/latest.bin");
    }

    #[test]
    fn test_deserialize_new_paths_and_impact_v2_fields() {
        let raw = r#"
            [impact]
            testmap_path = ".covy/state/t.bin"
            max_tests = 12
            target_coverage = 0.95
            stale_after_days = 7
            allow_stale = false
            test_id_strategy = "pytest"

            [paths]
            strip_prefix = ["/home/runner/work/repo/repo", "/__w/repo/repo"]
            ignore_globs = ["**/bazel-out/**"]
            case_sensitive = false

            [[paths.replace_prefix]]
            from = "/workspace"
            to = "."
        "#;
        let config: CovyConfig = toml::from_str(raw).unwrap();
        assert_eq!(config.impact.testmap_path, ".covy/state/t.bin");
        assert_eq!(config.impact.max_tests, 12);
        assert!((config.impact.target_coverage - 0.95).abs() < f64::EPSILON);
        assert_eq!(config.impact.stale_after_days, 7);
        assert!(!config.impact.allow_stale);
        assert_eq!(config.impact.test_id_strategy, "pytest");
        assert_eq!(config.paths.strip_prefix.len(), 2);
        assert_eq!(config.paths.replace_prefix.len(), 1);
        assert_eq!(config.paths.replace_prefix[0].from, "/workspace");
        assert_eq!(config.paths.replace_prefix[0].to, ".");
        assert_eq!(
            config.paths.ignore_globs,
            vec!["**/bazel-out/**".to_string()]
        );
        assert!(!config.paths.case_sensitive);
    }

    #[test]
    fn test_deserialize_legacy_path_mapping_still_supported() {
        let raw = r#"
            [path_mapping.rules]
            "/build/classes/" = "src/main/java/"
        "#;
        let config: CovyConfig = toml::from_str(raw).unwrap();
        assert_eq!(
            config.path_mapping.rules.get("/build/classes/"),
            Some(&"src/main/java/".to_string())
        );
    }
}
