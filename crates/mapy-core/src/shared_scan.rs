//! Experimental borrowed-content rebuild session for daemon composition.
//!
//! This module is available only with the non-default
//! `shared-repository-scan` feature. It owns map-specific filtering, scan-cache
//! behavior, derived metadata, the repository writer lock, and generation
//! preparation while allowing a coordinator to lend each file buffer.

use std::collections::BTreeSet;
use std::fs::Metadata;
use std::path::{Component, Path, PathBuf};

use suite_packet_core::CovyError;

use crate::generation::{
    acquire_writer_lock, prepare_repo_index_runtime_with_writer, GenerationWriterLock,
    PreparedRepoIndexRuntime,
};
use crate::runtime::repo_index_from_scans;
use crate::scan::{is_generated_or_vendor_path, is_source_file, is_test_path, RepoScanAccumulator};

/// Returns whether a map rebuild wants a path yielded by its repository walker.
///
/// Ignore-file, hidden-file, and traversal decisions remain the coordinator's
/// responsibility because they are properties of the walk rather than the
/// file format.
pub fn wants_path(relative_path: &str, include_tests: bool) -> bool {
    if !wants_traversal(relative_path) {
        return false;
    }
    let path = Path::new(relative_path);
    is_source_file(path) && (include_tests || !is_test_path(relative_path))
}

/// Returns whether the map scanner would traverse a repository-relative path.
///
/// A shared walker can use this predicate to preserve map-specific generated
/// and vendor pruning while still traversing paths needed by another index.
/// It is also the policy boundary for deciding whether a walker error would
/// have been observable to the standalone map scan.
pub fn wants_traversal(relative_path: &str) -> bool {
    is_normalized_repository_relative_path(relative_path)
        && !is_generated_or_vendor_path(relative_path)
}

fn is_normalized_repository_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && path.split('/').all(|component| !component.is_empty())
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// A map-specific full-rebuild session that borrows one content buffer at a time.
///
/// The session holds the repository generation writer lock until its prepared
/// handle is committed or dropped. `discovered_paths` must be the exact,
/// sorted set the map-specific walk would report; it controls cache eviction
/// even when metadata or content reads later fail.
pub struct RepoIndexScanSession {
    root: PathBuf,
    include_tests: bool,
    discovered: BTreeSet<String>,
    accumulator: RepoScanAccumulator,
    writer: GenerationWriterLock,
}

impl RepoIndexScanSession {
    /// Starts a locked map rebuild from a coordinator-owned discovery plan.
    ///
    /// # Errors
    ///
    /// Returns [`CovyError::Other`] when `root` does not exist,
    /// [`CovyError::PathMapping`] for a non-normalized or escaping path, and
    /// [`CovyError::Cache`] when the repository writer lock cannot be acquired.
    pub fn begin(
        root: &Path,
        include_tests: bool,
        discovered_paths: &[String],
    ) -> Result<Self, CovyError> {
        if !root.exists() {
            return Err(CovyError::Other(format!(
                "repo_root does not exist: {}",
                root.display()
            )));
        }
        if let Some(path) = discovered_paths
            .iter()
            .find(|path| !is_normalized_repository_relative_path(path))
        {
            return Err(CovyError::PathMapping(format!(
                "shared map scan path '{path}' must be normalized beneath the repository root"
            )));
        }
        let discovered = discovered_paths
            .iter()
            .filter(|path| wants_path(path, include_tests))
            .cloned()
            .collect::<BTreeSet<_>>();
        let source_paths = discovered.iter().cloned().collect::<Vec<_>>();
        let writer = acquire_writer_lock(root)?;
        Ok(Self {
            root: root.to_path_buf(),
            include_tests,
            discovered,
            accumulator: RepoScanAccumulator::new(root, &source_paths),
            writer,
        })
    }

    /// Returns the map-specific progress total for this discovery plan.
    pub fn total_files(&self) -> usize {
        self.discovered.len()
    }

    /// Borrows one successfully read map candidate and derives its cached metadata.
    ///
    /// Invalid UTF-8 retains the historical behavior: the file is omitted and
    /// any stale scan-cache entry is evicted.
    ///
    /// # Errors
    ///
    /// Returns [`CovyError::PathMapping`] when `relative_path` was not part of
    /// the immutable discovery plan.
    pub fn ingest(
        &mut self,
        relative_path: &str,
        metadata: &Metadata,
        bytes: &[u8],
    ) -> Result<(), CovyError> {
        if !self.discovered.contains(relative_path) {
            return Err(CovyError::PathMapping(format!(
                "shared map scan path '{relative_path}' was not in the discovery plan"
            )));
        }
        self.accumulator.ingest(relative_path, metadata, bytes);
        Ok(())
    }

    /// Builds and validates immutable artifacts without publishing the manifest.
    ///
    /// # Errors
    ///
    /// Returns [`CovyError::Cache`] for encoding, persistence, or generation
    /// validation failures.
    pub fn prepare(self) -> Result<PreparedRepoIndexRuntime, CovyError> {
        let snapshot = repo_index_from_scans(self.accumulator.finish(), self.include_tests);
        prepare_repo_index_runtime_with_writer(
            &self.root,
            self.include_tests,
            snapshot,
            self.writer,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use super::*;
    use crate::{load_repo_index_runtime, rebuild_repo_index_runtime};

    fn prepare_shared(root: &Path, paths: &[String]) -> PreparedRepoIndexRuntime {
        let mut session = RepoIndexScanSession::begin(root, true, paths).unwrap();
        for relative_path in paths {
            let path = root.join(relative_path);
            let metadata = fs::metadata(&path).unwrap();
            let bytes = fs::read(path).unwrap();
            session.ingest(relative_path, &metadata, &bytes).unwrap();
        }
        session.prepare().unwrap()
    }

    fn generation_artifact_snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
        let directory = root.join(".packet28").join("index").join("mapy-v1");
        fs::read_dir(directory)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().to_string_lossy().to_string(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn borrowed_scan_matches_the_standard_snapshot_and_rolls_back_until_commit() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(root.join("src/lib.rs"), b"pub fn visible() -> u64 { 7 }\n").unwrap();
        fs::write(root.join("tests/case.rs"), b"#[test] fn works() {}\n").unwrap();
        let paths = vec!["src/lib.rs".to_string(), "tests/case.rs".to_string()];

        let standard = rebuild_repo_index_runtime(root, true).unwrap();
        let expected = standard.materialize_snapshot().unwrap();
        let mut prepared = prepare_shared(root, &paths);

        assert_eq!(
            load_repo_index_runtime(root).unwrap().manifest.generation,
            standard.manifest.generation
        );
        prepared.publish().unwrap();
        assert!(
            load_repo_index_runtime(root).unwrap().manifest.generation
                > standard.manifest.generation
        );
        prepared.rollback().unwrap();
        assert_eq!(
            load_repo_index_runtime(root).unwrap().manifest.generation,
            standard.manifest.generation
        );

        prepared.publish().unwrap();
        let shared = prepared.commit().unwrap();
        assert_eq!(shared.materialize_snapshot().unwrap(), expected);
    }

    #[test]
    fn dropping_a_published_generation_restores_the_previous_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), b"pub struct Visible;\n").unwrap();
        let paths = vec!["src/lib.rs".to_string()];
        let standard = rebuild_repo_index_runtime(root, true).unwrap();

        let mut prepared = prepare_shared(root, &paths);
        prepared.publish().unwrap();
        drop(prepared);

        assert_eq!(
            load_repo_index_runtime(root).unwrap().manifest.generation,
            standard.manifest.generation
        );
    }

    #[test]
    fn dropping_prepared_repair_preserves_the_retained_recovery_generation() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), b"pub struct First;\n").unwrap();
        let paths = vec!["src/lib.rs".to_string()];
        let first = rebuild_repo_index_runtime(root, true).unwrap();
        fs::write(root.join("src/lib.rs"), b"pub struct Second;\n").unwrap();
        let second = rebuild_repo_index_runtime(root, true).unwrap();
        let index_parent = root.join(".packet28").join("index");
        let index_dir = index_parent.join("mapy-v1");
        let first_base = index_dir.join(format!("base-{:020}.bin", first.manifest.generation));
        let first_record =
            index_dir.join(format!("generation-{:020}.json", first.manifest.generation));
        let first_base_bytes = fs::read(&first_base).unwrap();
        let first_record_bytes = fs::read(&first_record).unwrap();
        fs::remove_file(index_parent.join(".mapy-v1.generation-high-water.json")).unwrap();
        fs::write(index_dir.join("manifest.json"), b"{").unwrap();

        let mut prepared = prepare_shared(root, &paths);
        assert!(prepared.manifest().generation > second.manifest.generation);
        prepared.publish().unwrap();
        drop(prepared);

        assert_eq!(fs::read(first_base).unwrap(), first_base_bytes);
        assert_eq!(fs::read(first_record).unwrap(), first_record_bytes);
        assert_eq!(
            load_repo_index_runtime(root).unwrap().manifest.generation,
            first.manifest.generation
        );
    }

    #[test]
    fn shared_prepare_at_generation_max_writes_no_generation_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), b"pub struct Current;\n").unwrap();
        let paths = vec!["src/lib.rs".to_string()];
        let current = rebuild_repo_index_runtime(root, true).unwrap();
        let high_water_path = root
            .join(".packet28")
            .join("index")
            .join(".mapy-v1.generation-high-water.json");
        fs::write(
            &high_water_path,
            format!("{{\"schema_version\":1,\"generation\":{}}}", u64::MAX),
        )
        .unwrap();
        let before = generation_artifact_snapshot(root);
        let counter_before = fs::read(&high_water_path).unwrap();
        let mut session = RepoIndexScanSession::begin(root, true, &paths).unwrap();
        let metadata = fs::metadata(root.join("src/lib.rs")).unwrap();
        let bytes = fs::read(root.join("src/lib.rs")).unwrap();
        session.ingest("src/lib.rs", &metadata, &bytes).unwrap();

        let error = session.prepare().err().expect("generation exhaustion");

        assert!(error.to_string().contains("exhausted"));
        assert_eq!(generation_artifact_snapshot(root), before);
        assert_eq!(fs::read(high_water_path).unwrap(), counter_before);
        assert_eq!(
            load_repo_index_runtime(root).unwrap().manifest.generation,
            current.manifest.generation
        );
    }

    #[test]
    fn failed_publication_restores_both_manifest_files_exactly() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), b"pub struct Visible;\n").unwrap();
        let paths = vec!["src/lib.rs".to_string()];
        rebuild_repo_index_runtime(root, true).unwrap();
        rebuild_repo_index_runtime(root, true).unwrap();
        let index_dir = root.join(".packet28").join("index").join("mapy-v1");
        let current_path = index_dir.join("manifest.json");
        let previous_path = index_dir.join("manifest.previous.json");
        let expected_current = fs::read(&current_path).unwrap();
        let expected_previous = fs::read(&previous_path).unwrap();
        let mut prepared = prepare_shared(root, &paths);

        let error = prepared
            .publish_with(|root, previous, current| {
                let index_dir = root.join(".packet28").join("index").join("mapy-v1");
                fs::write(
                    index_dir.join("manifest.json"),
                    serde_json::to_vec_pretty(current).unwrap(),
                )
                .unwrap();
                fs::write(
                    index_dir.join("manifest.previous.json"),
                    serde_json::to_vec_pretty(previous.unwrap()).unwrap(),
                )
                .unwrap();
                Err(CovyError::Cache("injected publication failure".to_string()))
            })
            .unwrap_err();

        assert!(error.to_string().contains("injected publication failure"));
        assert_eq!(fs::read(current_path).unwrap(), expected_current);
        assert_eq!(fs::read(previous_path).unwrap(), expected_previous);
    }

    #[test]
    fn discovery_plan_rejects_paths_outside_the_repository() {
        let directory = tempfile::tempdir().unwrap();
        let error =
            RepoIndexScanSession::begin(directory.path(), true, &["../outside.rs".to_string()])
                .err()
                .unwrap();

        assert!(matches!(error, CovyError::PathMapping(_)));
        assert!(!wants_path("../outside.rs", true));
        assert!(!wants_path(r"src\\outside.rs", true));
    }

    #[test]
    fn traversal_policy_exposes_generated_directory_pruning() {
        assert!(wants_traversal("src"));
        assert!(wants_traversal("vendor/src"));
        // The standalone walk observes the top-level directory entry before
        // pruning its children, so the shared policy preserves that boundary.
        assert!(wants_traversal("target"));
        assert!(!wants_traversal("target/debug/lib.rs"));
        assert!(!wants_traversal("nested/build/generated.rs"));
        assert!(!wants_traversal("scratch/.tmp-session/source.rs"));
        assert!(!wants_traversal("../outside"));
        assert!(!wants_traversal(r"src\\outside"));
    }
}
