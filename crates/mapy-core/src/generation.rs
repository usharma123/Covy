//! Incremental repository-index generations with manifest-last publication.
//!
//! Public writers serialize on a repository-local lock and reject stale
//! generation handles. Generation records bind every immutable artifact to a
//! BLAKE3 digest, and the atomically published manifest binds the canonical
//! generation record to its own digest. Best-effort pruning retains the current
//! and explicitly recoverable previous generation under normal filesystem
//! operation.
//!
//! Publication is process-crash atomic on filesystems that provide atomic
//! same-directory rename: artifacts are written to flushed temporary files and
//! the manifest is renamed last. Files and parent directories are not
//! `fsync`ed, so this module does not claim power-loss durability.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use suite_packet_core::CovyError;

use crate::runtime::{index_repo_path, normalize_path, MAP_CACHE_VERSION};
use crate::{
    build_repo_index_with_progress, RepoIndexFileEntry, RepoIndexSnapshot, RepoIndexUpdateSummary,
};

const REPO_INDEX_RUNTIME_SCHEMA_VERSION: u32 = 1;
const REPO_INDEX_DIR_NAME: &str = "mapy-v1";
const REPO_INDEX_MANIFEST_FILE: &str = "manifest.json";
const REPO_INDEX_PREVIOUS_MANIFEST_FILE: &str = "manifest.previous.json";
const REPO_INDEX_WRITER_LOCK_FILE: &str = ".mapy-v1.writer.lock";
const REPO_INDEX_COMPACTION_SEGMENTS: usize = 8;
const REPO_INDEX_SEGMENT_VERSION: u32 = 1;

static TEMP_FILE_NONCE: AtomicU64 = AtomicU64::new(0);

/// Metadata for an atomically published repository-index generation.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct RepoIndexRuntimeManifest {
    /// On-disk generation-record schema.
    pub schema_version: u32,
    /// Monotonically increasing publication generation.
    pub generation: u64,
    /// Whether test files are represented by this generation.
    pub include_tests: bool,
    /// Number of visible repository files.
    pub total_files: usize,
    /// Number of currently visible files owned by overlay segments.
    pub overlay_files: usize,
    /// Number of immutable overlay segments referenced by this generation.
    pub segment_count: usize,
    /// Digest authenticating the canonical generation record selected by this manifest.
    ///
    /// Recorded generations without this field are rejected and rebuilt
    /// because their artifact identities cannot be distinguished from a
    /// structurally valid stale generation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_record_digest: Option<String>,
    /// Lifecycle status (`missing`, `ready`, or `corrupt`).
    pub status: String,
    /// Newer corrupt generation skipped during recovery, when applicable.
    pub recovered_from_generation: Option<u64>,
    /// Validation failure that prevented the current generation from loading.
    pub last_error: Option<String>,
}

/// A cheap-to-clone read handle for one immutable repository-index generation.
///
/// Updates return a new runtime while existing readers retain their base and
/// segment `Arc`s. Repository-sized materialization occurs only during an
/// explicit snapshot request, a full rebuild, or threshold compaction.
#[derive(Debug, Clone, Default)]
pub struct RepoIndexRuntime {
    /// Metadata for the loaded or unavailable generation.
    pub manifest: RepoIndexRuntimeManifest,
    generation: Option<Arc<RepoIndexGeneration>>,
}

impl RepoIndexRuntime {
    /// Returns whether this handle owns a validated generation.
    pub fn is_loaded(&self) -> bool {
        self.generation.is_some()
    }

    /// Returns the visible entry for `path`, honoring the newest segment or tombstone.
    pub fn file(&self, path: &str) -> Option<&RepoIndexFileEntry> {
        let normalized = normalize_path(path);
        let generation = self.generation.as_ref()?;
        match generation.latest_by_path.get(&normalized) {
            Some(Some(owner)) => generation
                .segments
                .iter()
                .rev()
                .find(|segment| segment.generation == *owner)
                .and_then(|segment| segment.files.get(&normalized)),
            Some(None) => None,
            None => generation.base.files.get(&normalized),
        }
    }

    /// Visits every visible file without cloning the repository-sized base map.
    pub fn for_each_file(&self, mut visit: impl FnMut(&RepoIndexFileEntry)) {
        let Some(generation) = self.generation.as_ref() else {
            return;
        };
        for entry in generation.base.files.values() {
            if !generation.latest_by_path.contains_key(&entry.path) {
                visit(entry);
            }
        }
        for segment in &generation.segments {
            for entry in segment.files.values() {
                if generation.latest_by_path.get(&entry.path) == Some(&Some(segment.generation)) {
                    visit(entry);
                }
            }
        }
    }

    /// Materializes a compatibility [`RepoIndexSnapshot`].
    ///
    /// This operation intentionally clones visible entries and should be kept
    /// off incremental publication paths.
    pub fn materialize_snapshot(&self) -> Option<RepoIndexSnapshot> {
        self.generation.as_ref()?;
        let mut files = BTreeMap::new();
        self.for_each_file(|entry| {
            files.insert(entry.path.clone(), entry.clone());
        });
        Some(RepoIndexSnapshot {
            version: MAP_CACHE_VERSION,
            include_tests: self.manifest.include_tests,
            files,
        })
    }

    /// Returns true when two reader generations retain the same immutable base.
    pub fn shares_base_with(&self, other: &Self) -> bool {
        match (self.generation.as_ref(), other.generation.as_ref()) {
            (Some(left), Some(right)) => Arc::ptr_eq(&left.base, &right.base),
            _ => false,
        }
    }
}

/// A feature-gated, validated repository generation awaiting manifest publication.
///
/// The handle owns the repository writer lock. Dropping it before
/// [`Self::commit`] leaves the published manifest unchanged; dropping it after
/// [`Self::publish`] performs a best-effort rollback to the exact manifest
/// bytes observed before preparation.
#[cfg(feature = "shared-repository-scan")]
pub struct PreparedRepoIndexRuntime {
    root: PathBuf,
    _writer: GenerationWriterLock,
    previous: Option<RepoIndexRuntimeManifest>,
    previous_current_bytes: Option<Vec<u8>>,
    previous_previous_bytes: Option<Vec<u8>>,
    record: RepoIndexGenerationRecord,
    runtime: RepoIndexRuntime,
    published: bool,
    committed: bool,
}

#[cfg(feature = "shared-repository-scan")]
impl PreparedRepoIndexRuntime {
    /// Publishes this validated generation while retaining rollback ownership.
    ///
    /// # Errors
    ///
    /// Returns [`CovyError::Cache`] if this handle was already published or
    /// the manifest cannot be atomically replaced.
    pub fn publish(&mut self) -> Result<(), CovyError> {
        self.publish_with(publish_manifest)
    }

    pub(crate) fn publish_with<F>(&mut self, publish: F) -> Result<(), CovyError>
    where
        F: FnOnce(
            &Path,
            Option<&RepoIndexRuntimeManifest>,
            &RepoIndexRuntimeManifest,
        ) -> Result<(), CovyError>,
    {
        if self.published {
            return Err(cache_error(
                "prepared repository generation was already published",
            ));
        }
        self.published = true;
        match publish(
            &self.root,
            self.previous.as_ref(),
            &self.runtime.manifest,
        ) {
            Ok(()) => Ok(()),
            Err(publication) => match self.rollback() {
                Ok(()) => Err(publication),
                Err(rollback) => Err(cache_error(format!(
                    "repository generation publication failed ({publication}); restoring the pre-publication manifests also failed ({rollback})"
                ))),
            },
        }
    }

    /// Restores both repository manifest files to their pre-publication bytes.
    ///
    /// # Errors
    ///
    /// Returns [`CovyError::Cache`] if either manifest cannot be restored.
    pub fn rollback(&mut self) -> Result<(), CovyError> {
        if !self.published {
            return Ok(());
        }
        restore_optional_file(
            previous_manifest_path(&self.root),
            self.previous_previous_bytes.as_deref(),
        )?;
        restore_optional_file(
            manifest_path(&self.root),
            self.previous_current_bytes.as_deref(),
        )?;
        self.published = false;
        Ok(())
    }

    /// Finalizes a successfully paired publication and releases its writer lock.
    ///
    /// # Errors
    ///
    /// Returns [`CovyError::Cache`] if [`Self::publish`] has not succeeded.
    pub fn commit(mut self) -> Result<RepoIndexRuntime, CovyError> {
        if !self.published {
            return Err(cache_error(
                "prepared repository generation must be published before commit",
            ));
        }
        let _ = prune_generation_artifacts(&self.root, &self.record, self.previous.as_ref());
        self.committed = true;
        Ok(self.runtime.clone())
    }

    /// Returns metadata for the validated generation without publishing it.
    pub fn manifest(&self) -> &RepoIndexRuntimeManifest {
        &self.runtime.manifest
    }
}

#[cfg(feature = "shared-repository-scan")]
impl Drop for PreparedRepoIndexRuntime {
    fn drop(&mut self) {
        if self.published && !self.committed {
            let _ = self.rollback();
        }
    }
}

#[derive(Debug)]
struct RepoIndexGeneration {
    base: Arc<RepoIndexSnapshot>,
    base_file: String,
    base_digest: String,
    segments: Vec<Arc<RepoIndexSegment>>,
    segment_files: Vec<String>,
    segment_digests: Vec<String>,
    latest_by_path: BTreeMap<String, Option<u64>>,
}

#[derive(
    Debug, Clone, Serialize, Deserialize, Default, wincode::SchemaRead, wincode::SchemaWrite,
)]
#[serde(default)]
struct RepoIndexSegment {
    version: u32,
    generation: u64,
    files: BTreeMap<String, RepoIndexFileEntry>,
    tombstones: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RepoIndexGenerationRecord {
    schema_version: u32,
    generation: u64,
    base_file: String,
    base_digest: String,
    segment_files: Vec<String>,
    segment_digests: Vec<String>,
    manifest: RepoIndexRuntimeManifest,
}

/// Builds and manifest-last publishes a new immutable base generation.
///
/// The old manifest stays authoritative until every new artifact has been
/// written, flushed, reloaded, and validated. A repository-local exclusive
/// writer lock covers the build and publication.
///
/// # Errors
///
/// Returns [`CovyError::Other`] for repository scan failures and
/// [`CovyError::Cache`] for encoding, persistence, or validation failures.
pub fn rebuild_repo_index_runtime(
    root: &Path,
    include_tests: bool,
) -> Result<RepoIndexRuntime, CovyError> {
    rebuild_repo_index_runtime_with_progress(root, include_tests, |_, _| {})
}

/// Builds and atomically publishes a new immutable base generation while
/// reporting repository scan progress.
///
/// The callback receives `(indexed_files, total_files)` checkpoints from the
/// repository scanner. Publication remains manifest-last, so progress does not
/// expose a partially written generation to readers.
///
/// # Errors
///
/// Returns [`CovyError::Other`] for repository scan failures and
/// [`CovyError::Cache`] for encoding, persistence, or validation failures.
pub fn rebuild_repo_index_runtime_with_progress<F>(
    root: &Path,
    include_tests: bool,
    on_progress: F,
) -> Result<RepoIndexRuntime, CovyError>
where
    F: FnMut(usize, usize),
{
    let _writer = acquire_writer_lock(root)?;
    let snapshot = build_repo_index_with_progress(root, include_tests, on_progress)?;
    publish_rebuilt_runtime(root, include_tests, snapshot)
}

fn publish_rebuilt_runtime(
    root: &Path,
    include_tests: bool,
    snapshot: RepoIndexSnapshot,
) -> Result<RepoIndexRuntime, CovyError> {
    let previous = load_published_manifest(root).ok();
    let generation = next_generation(previous.as_ref(), None);
    let base_file = base_file_name(generation);
    let base_bytes = wincode::serialize(&snapshot)
        .map_err(|error| cache_error(format!("failed to encode repository base: {error}")))?;
    write_atomic(repo_index_dir(root).join(&base_file), &base_bytes)?;
    let base_digest = artifact_digest(&base_bytes);

    let mut manifest = RepoIndexRuntimeManifest {
        schema_version: REPO_INDEX_RUNTIME_SCHEMA_VERSION,
        generation,
        include_tests,
        total_files: snapshot.files.len(),
        overlay_files: 0,
        segment_count: 0,
        generation_record_digest: None,
        status: "ready".to_string(),
        recovered_from_generation: None,
        last_error: None,
    };
    let mut record = RepoIndexGenerationRecord {
        schema_version: REPO_INDEX_RUNTIME_SCHEMA_VERSION,
        generation,
        base_file,
        base_digest,
        segment_files: Vec::new(),
        segment_digests: Vec::new(),
        manifest: manifest.clone(),
    };
    bind_generation_record(&mut manifest, &mut record)?;
    persist_generation_record(root, &record)?;
    let runtime = load_generation(root, &manifest)?;
    publish_manifest(root, previous.as_ref(), &manifest)?;
    let _ = prune_generation_artifacts(root, &record, previous.as_ref());
    Ok(runtime)
}

#[cfg(feature = "shared-repository-scan")]
pub(crate) fn prepare_repo_index_runtime_with_writer(
    root: &Path,
    include_tests: bool,
    snapshot: RepoIndexSnapshot,
    writer: GenerationWriterLock,
) -> Result<PreparedRepoIndexRuntime, CovyError> {
    let previous_current_bytes = read_optional_file(&manifest_path(root))?;
    let previous_previous_bytes = read_optional_file(&previous_manifest_path(root))?;
    let previous = load_published_manifest(root).ok();
    let generation = next_generation(previous.as_ref(), None);
    let base_file = base_file_name(generation);
    let base_bytes = wincode::serialize(&snapshot)
        .map_err(|error| cache_error(format!("failed to encode repository base: {error}")))?;
    write_atomic(repo_index_dir(root).join(&base_file), &base_bytes)?;
    let base_digest = artifact_digest(&base_bytes);
    let mut manifest = RepoIndexRuntimeManifest {
        schema_version: REPO_INDEX_RUNTIME_SCHEMA_VERSION,
        generation,
        include_tests,
        total_files: snapshot.files.len(),
        overlay_files: 0,
        segment_count: 0,
        generation_record_digest: None,
        status: "ready".to_string(),
        recovered_from_generation: None,
        last_error: None,
    };
    let mut record = RepoIndexGenerationRecord {
        schema_version: REPO_INDEX_RUNTIME_SCHEMA_VERSION,
        generation,
        base_file,
        base_digest,
        segment_files: Vec::new(),
        segment_digests: Vec::new(),
        manifest: manifest.clone(),
    };
    bind_generation_record(&mut manifest, &mut record)?;
    persist_generation_record(root, &record)?;
    let runtime = load_generation(root, &manifest)?;
    Ok(PreparedRepoIndexRuntime {
        root: root.to_path_buf(),
        _writer: writer,
        previous,
        previous_current_bytes,
        previous_previous_bytes,
        record,
        runtime,
        published: false,
        committed: false,
    })
}

/// Removes every persisted repository-index generation.
///
/// Existing [`RepoIndexRuntime`] handles remain readable because they own their
/// immutable generation data independently of the on-disk artifacts.
///
/// # Errors
///
/// Returns [`CovyError::Cache`] when the generation directory cannot be
/// removed.
pub fn clear_repo_index_runtime(root: &Path) -> Result<(), CovyError> {
    let _writer = acquire_writer_lock(root)?;
    let path = repo_index_dir(root);
    if path.exists() {
        fs::remove_dir_all(&path).map_err(|error| {
            cache_error(format!(
                "failed to remove repository index directory '{}': {error}",
                path.display()
            ))
        })?;
    }
    Ok(())
}

/// Publishes changed paths as one immutable delta segment.
///
/// At eight referenced segments, live overlay entries are compacted into one
/// segment. The immutable base remains shared across old and new readers. A
/// repository-local lock serializes publishers, and an update from a stale
/// runtime returns an explicit generation-conflict cache error.
///
/// # Errors
///
/// Returns [`CovyError::Cache`] if `current` is unloaded or if an artifact
/// cannot be encoded, flushed, validated, or published. File metadata/content
/// failures use the existing repository-index error behavior.
pub fn update_repo_index_runtime(
    root: &Path,
    current: &RepoIndexRuntime,
    changed_paths: &[String],
    include_tests: bool,
) -> Result<(RepoIndexRuntime, RepoIndexUpdateSummary), CovyError> {
    let loaded = current
        .generation
        .as_ref()
        .ok_or_else(|| cache_error("repository index runtime is not loaded"))?;
    if changed_paths.is_empty() {
        return Ok((
            current.clone(),
            RepoIndexUpdateSummary {
                indexed_files: 0,
                removed_files: 0,
                changed_paths: Vec::new(),
            },
        ));
    }
    let normalized = normalize_changed_paths(root, changed_paths)?;
    if current.manifest.include_tests != include_tests {
        let rebuilt = rebuild_repo_index_runtime(root, include_tests)?;
        return Ok((
            rebuilt,
            RepoIndexUpdateSummary {
                indexed_files: 0,
                removed_files: 0,
                changed_paths: normalized,
            },
        ));
    }
    let _writer = acquire_writer_lock(root)?;
    let published = load_published_manifest(root)?;
    if published.generation != current.manifest.generation {
        return Err(cache_error(format!(
            "repository index generation conflict: caller has {}, published generation is {}",
            current.manifest.generation, published.generation
        )));
    }
    let generation = published.generation.saturating_add(1);
    let mut segment = RepoIndexSegment {
        version: REPO_INDEX_SEGMENT_VERSION,
        generation,
        ..RepoIndexSegment::default()
    };
    let mut indexed_files = 0;
    let mut removed_files = 0;
    for path in &normalized {
        match index_repo_path(root, path, include_tests)? {
            Some(entry) => {
                segment.files.insert(path.clone(), entry);
                indexed_files += 1;
            }
            None => {
                segment.tombstones.insert(path.clone());
                if current.file(path).is_some() {
                    removed_files += 1;
                }
            }
        }
    }
    validate_segment(&segment)?;

    let mut segments = loaded.segments.clone();
    let mut segment_files = loaded.segment_files.clone();
    let mut segment_digests = loaded.segment_digests.clone();
    let segment_file = segment_file_name(generation);
    let segment_digest = persist_segment(root, &segment_file, &segment)?;
    let segment = Arc::new(segment);
    segment_files.push(segment_file);
    segment_digests.push(segment_digest);

    let mut latest_by_path = loaded.latest_by_path.clone();
    apply_segment_resolution(&segment, &mut latest_by_path);
    segments.push(segment);

    if segments.len() >= REPO_INDEX_COMPACTION_SEGMENTS {
        let compacted = compact_segments(generation, &segments, &latest_by_path);
        let compacted_file = compacted_segment_file_name(generation);
        let compacted_digest = persist_segment(root, &compacted_file, &compacted)?;
        let compacted = Arc::new(compacted);
        latest_by_path.clear();
        apply_segment_resolution(&compacted, &mut latest_by_path);
        segments = vec![compacted];
        segment_files = vec![compacted_file];
        segment_digests = vec![compacted_digest];
    }

    let total_files = visible_file_count(&loaded.base, &latest_by_path);
    let overlay_files = latest_by_path
        .values()
        .filter(|owner| owner.is_some())
        .count();
    let mut manifest = RepoIndexRuntimeManifest {
        schema_version: REPO_INDEX_RUNTIME_SCHEMA_VERSION,
        generation,
        include_tests,
        total_files,
        overlay_files,
        segment_count: segments.len(),
        generation_record_digest: None,
        status: "ready".to_string(),
        recovered_from_generation: None,
        last_error: None,
    };
    let mut record = RepoIndexGenerationRecord {
        schema_version: REPO_INDEX_RUNTIME_SCHEMA_VERSION,
        generation,
        base_file: loaded.base_file.clone(),
        base_digest: loaded.base_digest.clone(),
        segment_files: segment_files.clone(),
        segment_digests: segment_digests.clone(),
        manifest: manifest.clone(),
    };
    bind_generation_record(&mut manifest, &mut record)?;
    persist_generation_record(root, &record)?;

    let runtime = RepoIndexRuntime {
        manifest: manifest.clone(),
        generation: Some(Arc::new(RepoIndexGeneration {
            base: Arc::clone(&loaded.base),
            base_file: loaded.base_file.clone(),
            base_digest: loaded.base_digest.clone(),
            segments,
            segment_files,
            segment_digests,
            latest_by_path,
        })),
    };
    validate_runtime(&runtime)?;
    publish_manifest(root, Some(&current.manifest), &manifest)?;
    let _ = prune_generation_artifacts(root, &record, Some(&current.manifest));
    Ok((
        runtime,
        RepoIndexUpdateSummary {
            indexed_files,
            removed_files,
            changed_paths: normalized,
        },
    ))
}

/// Loads the current generation, falling back only to the explicitly retained
/// previous manifest when the current generation is corrupt.
///
/// Unpublished generation artifacts are deliberately ignored.
///
/// # Errors
///
/// Returns [`CovyError::Cache`] only when both current and retained previous
/// generation metadata cannot be read. Referenced-artifact corruption is
/// represented by an unloaded runtime carrying the validation reason.
pub fn load_repo_index_runtime(root: &Path) -> Result<RepoIndexRuntime, CovyError> {
    let current = match load_published_manifest(root) {
        Ok(manifest) => manifest,
        Err(current_error) => {
            return recover_previous(root, None, current_error);
        }
    };
    if current.schema_version == 0 {
        return Ok(RepoIndexRuntime {
            manifest: RepoIndexRuntimeManifest {
                status: "missing".to_string(),
                ..RepoIndexRuntimeManifest::default()
            },
            generation: None,
        });
    }
    match load_generation(root, &current) {
        Ok(runtime) => Ok(runtime),
        Err(current_error) => recover_previous(root, Some(current.generation), current_error),
    }
}

fn recover_previous(
    root: &Path,
    failed_generation: Option<u64>,
    current_error: CovyError,
) -> Result<RepoIndexRuntime, CovyError> {
    let previous = load_previous_manifest(root);
    if let Ok(previous) = previous {
        if let Ok(mut runtime) = load_generation(root, &previous) {
            runtime.manifest.recovered_from_generation = failed_generation;
            runtime.manifest.last_error = Some(current_error.to_string());
            return Ok(runtime);
        }
    }
    Ok(RepoIndexRuntime {
        manifest: RepoIndexRuntimeManifest {
            schema_version: REPO_INDEX_RUNTIME_SCHEMA_VERSION,
            generation: failed_generation.unwrap_or_default(),
            status: "corrupt".to_string(),
            recovered_from_generation: None,
            last_error: Some(current_error.to_string()),
            ..RepoIndexRuntimeManifest::default()
        },
        generation: None,
    })
}

fn load_generation(
    root: &Path,
    expected_manifest: &RepoIndexRuntimeManifest,
) -> Result<RepoIndexRuntime, CovyError> {
    validate_manifest(expected_manifest)?;
    let record_path =
        repo_index_dir(root).join(generation_record_file_name(expected_manifest.generation));
    let raw = fs::read(&record_path).map_err(|error| {
        cache_error(format!(
            "failed to read generation record '{}': {error}",
            record_path.display()
        ))
    })?;
    let record = serde_json::from_slice::<RepoIndexGenerationRecord>(&raw).map_err(|error| {
        cache_error(format!(
            "failed to decode generation record '{}': {error}",
            record_path.display()
        ))
    })?;
    if record.schema_version != REPO_INDEX_RUNTIME_SCHEMA_VERSION
        || record.generation != expected_manifest.generation
        || record.manifest != durable_manifest(expected_manifest)
        || record.base_digest.is_empty()
        || record.segment_files.len() != record.segment_digests.len()
        || record.segment_digests.iter().any(String::is_empty)
    {
        return Err(cache_error(format!(
            "generation record '{}' does not match its published manifest or digest metadata",
            record_path.display()
        )));
    }
    let expected_record_digest = expected_manifest
        .generation_record_digest
        .as_deref()
        .ok_or_else(|| {
            cache_error(format!(
                "published repository generation {} does not authenticate its generation record",
                expected_manifest.generation
            ))
        })?;
    let actual_record_digest = generation_record_digest(&record)?;
    if actual_record_digest != expected_record_digest {
        return Err(cache_error(format!(
            "generation record '{}' failed digest validation (expected {expected_record_digest}, found {actual_record_digest})",
            record_path.display()
        )));
    }
    validate_artifact_name(&record.base_file)?;
    let base_path = repo_index_dir(root).join(&record.base_file);
    let base_raw = fs::read(&base_path).map_err(|error| {
        cache_error(format!(
            "failed to read repository base '{}': {error}",
            base_path.display()
        ))
    })?;
    verify_artifact_digest(&base_path, &base_raw, &record.base_digest)?;
    let base = wincode::deserialize::<RepoIndexSnapshot>(&base_raw).map_err(|error| {
        cache_error(format!(
            "failed to decode repository base '{}': {error}",
            base_path.display()
        ))
    })?;
    validate_base(&base, expected_manifest.include_tests)?;

    let mut segments = Vec::with_capacity(record.segment_files.len());
    let mut latest_by_path = BTreeMap::new();
    let mut previous_generation = None;
    let mut unique_files = BTreeSet::new();
    for (file_name, expected_digest) in record.segment_files.iter().zip(&record.segment_digests) {
        validate_artifact_name(file_name)?;
        if !unique_files.insert(file_name) {
            return Err(cache_error(format!(
                "generation record '{}' references duplicate segment '{file_name}'",
                record_path.display()
            )));
        }
        let path = repo_index_dir(root).join(file_name);
        let raw = fs::read(&path).map_err(|error| {
            cache_error(format!(
                "failed to read repository segment '{}': {error}",
                path.display()
            ))
        })?;
        verify_artifact_digest(&path, &raw, expected_digest)?;
        let segment = wincode::deserialize::<RepoIndexSegment>(&raw).map_err(|error| {
            cache_error(format!(
                "failed to decode repository segment '{}': {error}",
                path.display()
            ))
        })?;
        validate_segment(&segment)?;
        if previous_generation.is_some_and(|previous| segment.generation <= previous)
            || segment.generation > expected_manifest.generation
        {
            return Err(cache_error(format!(
                "generation record '{}' has non-increasing or future segment generation {}",
                record_path.display(),
                segment.generation
            )));
        }
        previous_generation = Some(segment.generation);
        apply_segment_resolution(&segment, &mut latest_by_path);
        segments.push(Arc::new(segment));
    }
    let generation = RepoIndexGeneration {
        base: Arc::new(base),
        base_file: record.base_file,
        base_digest: record.base_digest,
        segments,
        segment_files: record.segment_files,
        segment_digests: record.segment_digests,
        latest_by_path,
    };
    let runtime = RepoIndexRuntime {
        manifest: expected_manifest.clone(),
        generation: Some(Arc::new(generation)),
    };
    validate_runtime(&runtime)?;
    Ok(runtime)
}

fn validate_manifest(manifest: &RepoIndexRuntimeManifest) -> Result<(), CovyError> {
    if manifest.schema_version != REPO_INDEX_RUNTIME_SCHEMA_VERSION {
        return Err(cache_error(format!(
            "repository index schema mismatch: found {}, expected {}",
            manifest.schema_version, REPO_INDEX_RUNTIME_SCHEMA_VERSION
        )));
    }
    if manifest.status != "ready" || manifest.generation == 0 {
        return Err(cache_error(format!(
            "repository index generation {} is not ready",
            manifest.generation
        )));
    }
    Ok(())
}

fn validate_base(base: &RepoIndexSnapshot, include_tests: bool) -> Result<(), CovyError> {
    if base.version == 0 || base.include_tests != include_tests {
        return Err(cache_error(
            "repository base has an invalid version or include-tests policy",
        ));
    }
    for (path, entry) in &base.files {
        if path.is_empty() || entry.path != *path {
            return Err(cache_error(format!(
                "repository base entry key '{path}' does not match path '{}'",
                entry.path
            )));
        }
    }
    Ok(())
}

fn validate_segment(segment: &RepoIndexSegment) -> Result<(), CovyError> {
    if segment.version != REPO_INDEX_SEGMENT_VERSION || segment.generation == 0 {
        return Err(cache_error(format!(
            "repository segment generation {} has invalid version {}",
            segment.generation, segment.version
        )));
    }
    for (path, entry) in &segment.files {
        if path.is_empty() || entry.path != *path {
            return Err(cache_error(format!(
                "repository segment entry key '{path}' does not match path '{}'",
                entry.path
            )));
        }
        if segment.tombstones.contains(path) {
            return Err(cache_error(format!(
                "repository segment contains both an entry and tombstone for '{path}'"
            )));
        }
    }
    if segment.tombstones.iter().any(String::is_empty) {
        return Err(cache_error(
            "repository segment contains an empty tombstone path",
        ));
    }
    Ok(())
}

fn validate_runtime(runtime: &RepoIndexRuntime) -> Result<(), CovyError> {
    let loaded = runtime
        .generation
        .as_ref()
        .ok_or_else(|| cache_error("repository generation is absent"))?;
    if loaded.segments.len() != runtime.manifest.segment_count {
        return Err(cache_error(format!(
            "repository manifest declares {} segments but {} loaded",
            runtime.manifest.segment_count,
            loaded.segments.len()
        )));
    }
    if loaded.segments.len() != loaded.segment_files.len()
        || loaded.segments.len() != loaded.segment_digests.len()
        || loaded.base_digest.is_empty()
        || loaded.segment_digests.iter().any(String::is_empty)
    {
        return Err(cache_error(
            "repository generation has inconsistent artifact digest metadata",
        ));
    }
    let total_files = visible_file_count(&loaded.base, &loaded.latest_by_path);
    let overlay_files = loaded
        .latest_by_path
        .values()
        .filter(|owner| owner.is_some())
        .count();
    if total_files != runtime.manifest.total_files
        || overlay_files != runtime.manifest.overlay_files
    {
        return Err(cache_error(format!(
            "repository manifest counts do not match loaded generation (files {}/{}, overlay {}/{})",
            runtime.manifest.total_files,
            total_files,
            runtime.manifest.overlay_files,
            overlay_files
        )));
    }
    Ok(())
}

fn visible_file_count(
    base: &RepoIndexSnapshot,
    latest_by_path: &BTreeMap<String, Option<u64>>,
) -> usize {
    let shadowed_base = latest_by_path
        .keys()
        .filter(|path| base.files.contains_key(*path))
        .count();
    let visible_overlay = latest_by_path
        .values()
        .filter(|owner| owner.is_some())
        .count();
    base.files
        .len()
        .saturating_sub(shadowed_base)
        .saturating_add(visible_overlay)
}

fn compact_segments(
    generation: u64,
    segments: &[Arc<RepoIndexSegment>],
    latest_by_path: &BTreeMap<String, Option<u64>>,
) -> RepoIndexSegment {
    let mut compacted = RepoIndexSegment {
        version: REPO_INDEX_SEGMENT_VERSION,
        generation,
        ..RepoIndexSegment::default()
    };
    for (path, owner) in latest_by_path {
        match owner {
            Some(owner) => {
                if let Some(entry) = segments
                    .iter()
                    .rev()
                    .find(|segment| segment.generation == *owner)
                    .and_then(|segment| segment.files.get(path))
                {
                    compacted.files.insert(path.clone(), entry.clone());
                }
            }
            None => {
                compacted.tombstones.insert(path.clone());
            }
        }
    }
    compacted
}

fn apply_segment_resolution(
    segment: &RepoIndexSegment,
    latest_by_path: &mut BTreeMap<String, Option<u64>>,
) {
    for path in segment.files.keys() {
        latest_by_path.insert(path.clone(), Some(segment.generation));
    }
    for path in &segment.tombstones {
        latest_by_path.insert(path.clone(), None);
    }
}

fn normalize_changed_paths(root: &Path, paths: &[String]) -> Result<Vec<String>, CovyError> {
    let canonical_root = fs::canonicalize(root).map_err(|error| {
        CovyError::PathMapping(format!(
            "cannot resolve repository root '{}': {error}",
            root.display()
        ))
    })?;
    let mut normalized = BTreeSet::new();
    for raw in paths {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.contains('\n') {
            return Err(invalid_changed_path(raw));
        }
        let input = Path::new(trimmed);
        let relative = if input.is_absolute() {
            if let Ok(stripped) = input.strip_prefix(root) {
                stripped.to_path_buf()
            } else if input.exists() {
                fs::canonicalize(input)
                    .ok()
                    .and_then(|path| {
                        path.strip_prefix(&canonical_root)
                            .ok()
                            .map(Path::to_path_buf)
                    })
                    .ok_or_else(|| invalid_changed_path(raw))?
            } else {
                return Err(invalid_changed_path(raw));
            }
        } else {
            input.to_path_buf()
        };
        let mut safe = PathBuf::new();
        for component in relative.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(part) => safe.push(part),
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(invalid_changed_path(raw));
                }
            }
        }
        if safe.as_os_str().is_empty() {
            continue;
        }
        let candidate = root.join(&safe);
        if candidate.exists() {
            let canonical_candidate =
                fs::canonicalize(&candidate).map_err(|_| invalid_changed_path(raw))?;
            if !canonical_candidate.starts_with(&canonical_root) {
                return Err(invalid_changed_path(raw));
            }
        }
        normalized.insert(normalize_path(&safe.to_string_lossy()));
    }
    Ok(normalized.into_iter().collect())
}

fn invalid_changed_path(path: &str) -> CovyError {
    CovyError::PathMapping(format!(
        "changed path '{path}' must resolve beneath the repository root"
    ))
}

fn persist_segment(
    root: &Path,
    file_name: &str,
    segment: &RepoIndexSegment,
) -> Result<String, CovyError> {
    let encoded = wincode::serialize(segment)
        .map_err(|error| cache_error(format!("failed to encode repository segment: {error}")))?;
    write_atomic(repo_index_dir(root).join(file_name), &encoded)?;
    let decoded = wincode::deserialize::<RepoIndexSegment>(&encoded)
        .map_err(|error| cache_error(format!("failed to validate repository segment: {error}")))?;
    validate_segment(&decoded)?;
    Ok(artifact_digest(&encoded))
}

fn persist_generation_record(
    root: &Path,
    record: &RepoIndexGenerationRecord,
) -> Result<(), CovyError> {
    let encoded = serde_json::to_vec_pretty(record).map_err(|error| {
        cache_error(format!(
            "failed to encode repository generation record: {error}"
        ))
    })?;
    write_atomic(
        repo_index_dir(root).join(generation_record_file_name(record.generation)),
        &encoded,
    )
}

fn bind_generation_record(
    manifest: &mut RepoIndexRuntimeManifest,
    record: &mut RepoIndexGenerationRecord,
) -> Result<(), CovyError> {
    manifest.generation_record_digest = None;
    record.manifest = manifest.clone();
    manifest.generation_record_digest = Some(generation_record_digest(record)?);
    record.manifest = manifest.clone();
    Ok(())
}

fn generation_record_digest(record: &RepoIndexGenerationRecord) -> Result<String, CovyError> {
    let mut canonical = record.clone();
    canonical.manifest.generation_record_digest = None;
    let encoded = serde_json::to_vec(&canonical).map_err(|error| {
        cache_error(format!(
            "failed to encode canonical repository generation record: {error}"
        ))
    })?;
    Ok(artifact_digest(&encoded))
}

fn publish_manifest(
    root: &Path,
    previous: Option<&RepoIndexRuntimeManifest>,
    current: &RepoIndexRuntimeManifest,
) -> Result<(), CovyError> {
    if let Some(previous) = previous.filter(|manifest| manifest.generation > 0) {
        let encoded = serde_json::to_vec_pretty(&durable_manifest(previous)).map_err(|error| {
            cache_error(format!(
                "failed to encode previous repository manifest: {error}"
            ))
        })?;
        write_atomic(previous_manifest_path(root), &encoded)?;
    }
    let encoded = serde_json::to_vec_pretty(current)
        .map_err(|error| cache_error(format!("failed to encode repository manifest: {error}")))?;
    write_atomic(manifest_path(root), &encoded)
}

fn durable_manifest(manifest: &RepoIndexRuntimeManifest) -> RepoIndexRuntimeManifest {
    let mut durable = manifest.clone();
    durable.recovered_from_generation = None;
    durable.last_error = None;
    durable
}

fn load_published_manifest(root: &Path) -> Result<RepoIndexRuntimeManifest, CovyError> {
    let path = manifest_path(root);
    if !path.exists() {
        return Ok(RepoIndexRuntimeManifest::default());
    }
    load_manifest_path(&path)
}

fn load_previous_manifest(root: &Path) -> Result<RepoIndexRuntimeManifest, CovyError> {
    load_manifest_path(&previous_manifest_path(root))
}

fn load_manifest_path(path: &Path) -> Result<RepoIndexRuntimeManifest, CovyError> {
    let raw = fs::read(path).map_err(|error| {
        cache_error(format!(
            "failed to read repository manifest '{}': {error}",
            path.display()
        ))
    })?;
    serde_json::from_slice(&raw).map_err(|error| {
        cache_error(format!(
            "failed to decode repository manifest '{}': {error}",
            path.display()
        ))
    })
}

fn next_generation(
    persisted: Option<&RepoIndexRuntimeManifest>,
    loaded_generation: Option<u64>,
) -> u64 {
    persisted
        .map_or(0, |manifest| manifest.generation)
        .max(loaded_generation.unwrap_or_default())
        .saturating_add(1)
}

pub(crate) struct GenerationWriterLock(File);

impl Drop for GenerationWriterLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

pub(crate) fn acquire_writer_lock(root: &Path) -> Result<GenerationWriterLock, CovyError> {
    let parent = root.join(".packet28").join("index");
    fs::create_dir_all(&parent).map_err(|error| {
        cache_error(format!(
            "failed to create repository index parent '{}': {error}",
            parent.display()
        ))
    })?;
    let path = parent.join(REPO_INDEX_WRITER_LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| {
            cache_error(format!(
                "failed to open repository index writer lock '{}': {error}",
                path.display()
            ))
        })?;
    FileExt::lock_exclusive(&file).map_err(|error| {
        cache_error(format!(
            "failed to acquire repository index writer lock '{}': {error}",
            path.display()
        ))
    })?;
    Ok(GenerationWriterLock(file))
}

#[cfg(feature = "shared-repository-scan")]
fn read_optional_file(path: &Path) -> Result<Option<Vec<u8>>, CovyError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(cache_error(format!(
            "failed to preserve index manifest '{}': {error}",
            path.display()
        ))),
    }
}

#[cfg(feature = "shared-repository-scan")]
fn restore_optional_file(path: PathBuf, bytes: Option<&[u8]>) -> Result<(), CovyError> {
    match bytes {
        Some(bytes) => write_atomic(path, bytes),
        None => match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(cache_error(format!(
                "failed to remove rolled-back manifest '{}': {error}",
                path.display()
            ))),
        },
    }
}

fn artifact_digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn verify_artifact_digest(path: &Path, bytes: &[u8], expected: &str) -> Result<(), CovyError> {
    let actual = artifact_digest(bytes);
    if actual != expected {
        return Err(cache_error(format!(
            "repository index artifact '{}' failed digest validation (expected {expected}, found {actual})",
            path.display()
        )));
    }
    Ok(())
}

fn prune_generation_artifacts(
    root: &Path,
    current: &RepoIndexGenerationRecord,
    previous: Option<&RepoIndexRuntimeManifest>,
) -> Result<(), CovyError> {
    let mut retained = BTreeSet::from([
        REPO_INDEX_MANIFEST_FILE.to_string(),
        REPO_INDEX_PREVIOUS_MANIFEST_FILE.to_string(),
        generation_record_file_name(current.generation),
        current.base_file.clone(),
    ]);
    retained.extend(current.segment_files.iter().cloned());
    if let Some(previous) = previous.filter(|manifest| manifest.generation > 0) {
        let record = load_generation_record(root, previous.generation)?;
        retained.insert(generation_record_file_name(record.generation));
        retained.insert(record.base_file);
        retained.extend(record.segment_files);
    }
    let directory = repo_index_dir(root);
    for entry in fs::read_dir(&directory).map_err(|error| {
        cache_error(format!(
            "failed to inspect repository index directory '{}': {error}",
            directory.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            cache_error(format!(
                "failed to inspect repository index entry in '{}': {error}",
                directory.display()
            ))
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if is_managed_generation_artifact(&name) && !retained.contains(&name) {
            fs::remove_file(entry.path()).map_err(|error| {
                cache_error(format!(
                    "failed to prune repository index artifact '{}': {error}",
                    entry.path().display()
                ))
            })?;
        }
    }
    Ok(())
}

fn load_generation_record(
    root: &Path,
    generation: u64,
) -> Result<RepoIndexGenerationRecord, CovyError> {
    let path = repo_index_dir(root).join(generation_record_file_name(generation));
    let raw = fs::read(&path).map_err(|error| {
        cache_error(format!(
            "failed to read repository generation record '{}': {error}",
            path.display()
        ))
    })?;
    serde_json::from_slice(&raw).map_err(|error| {
        cache_error(format!(
            "failed to decode repository generation record '{}': {error}",
            path.display()
        ))
    })
}

fn is_managed_generation_artifact(name: &str) -> bool {
    (name.starts_with("generation-") && name.ends_with(".json"))
        || (name.starts_with("base-") && name.ends_with(".bin"))
        || (name.starts_with("segment-") && name.ends_with(".bin"))
        || (name.starts_with('.') && name.ends_with(".tmp"))
}

fn write_atomic(path: PathBuf, bytes: &[u8]) -> Result<(), CovyError> {
    let parent = path
        .parent()
        .ok_or_else(|| cache_error(format!("artifact '{}' has no parent", path.display())))?;
    fs::create_dir_all(parent).map_err(|error| {
        cache_error(format!(
            "failed to create repository index directory '{}': {error}",
            parent.display()
        ))
    })?;
    let nonce = TEMP_FILE_NONCE.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("artifact"),
        std::process::id(),
        nonce
    ));
    let result = (|| -> Result<(), CovyError> {
        let mut file = File::create(&tmp).map_err(|error| {
            cache_error(format!(
                "failed to create temporary artifact '{}': {error}",
                tmp.display()
            ))
        })?;
        file.write_all(bytes).map_err(|error| {
            cache_error(format!(
                "failed to write temporary artifact '{}': {error}",
                tmp.display()
            ))
        })?;
        file.flush().map_err(|error| {
            cache_error(format!(
                "failed to flush temporary artifact '{}': {error}",
                tmp.display()
            ))
        })?;
        drop(file);
        fs::rename(&tmp, &path).map_err(|error| {
            cache_error(format!(
                "failed to publish repository artifact '{}': {error}",
                path.display()
            ))
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

fn validate_artifact_name(name: &str) -> Result<(), CovyError> {
    let path = Path::new(name);
    if name.is_empty() || path.is_absolute() || path.components().count() != 1 {
        return Err(cache_error(format!(
            "repository generation references invalid artifact name '{name}'"
        )));
    }
    Ok(())
}

fn repo_index_dir(root: &Path) -> PathBuf {
    root.join(".packet28")
        .join("index")
        .join(REPO_INDEX_DIR_NAME)
}

fn manifest_path(root: &Path) -> PathBuf {
    repo_index_dir(root).join(REPO_INDEX_MANIFEST_FILE)
}

fn previous_manifest_path(root: &Path) -> PathBuf {
    repo_index_dir(root).join(REPO_INDEX_PREVIOUS_MANIFEST_FILE)
}

fn generation_record_file_name(generation: u64) -> String {
    format!("generation-{generation:020}.json")
}

fn base_file_name(generation: u64) -> String {
    format!("base-{generation:020}.bin")
}

fn segment_file_name(generation: u64) -> String {
    format!("segment-{generation:020}.bin")
}

fn compacted_segment_file_name(generation: u64) -> String {
    format!("segment-{generation:020}-compacted.bin")
}

fn cache_error(message: impl Into<String>) -> CovyError {
    CovyError::Cache(message.into())
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;

    use super::*;
    use crate::{build_repo_index, update_repo_index};

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("src")).expect("src");
        fs::write(
            dir.path().join("src/a.rs"),
            "pub fn alpha() -> usize { 1 }\n",
        )
        .expect("a");
        fs::write(
            dir.path().join("src/b.rs"),
            "pub fn beta() -> usize { 2 }\n",
        )
        .expect("b");
        dir
    }

    fn current_record(root: &Path) -> RepoIndexGenerationRecord {
        let manifest = load_published_manifest(root).expect("manifest");
        let raw =
            fs::read(repo_index_dir(root).join(generation_record_file_name(manifest.generation)))
                .expect("record");
        serde_json::from_slice(&raw).expect("decode record")
    }

    #[test]
    fn incremental_update_and_delete_match_legacy_snapshot() {
        let dir = fixture();
        let root = dir.path();
        let mut legacy = build_repo_index(root, true).expect("legacy");
        let runtime = rebuild_repo_index_runtime(root, true).expect("runtime");

        fs::write(
            root.join("src/a.rs"),
            "pub fn alpha() -> usize { 3 }\npub fn gamma() {}\n",
        )
        .expect("update");
        fs::remove_file(root.join("src/b.rs")).expect("delete");
        fs::write(root.join("src/c.rs"), "pub struct Charlie;\n").expect("add");
        let paths = vec![
            "src/a.rs".to_string(),
            "src/b.rs".to_string(),
            "src/c.rs".to_string(),
        ];

        update_repo_index(root, &mut legacy, &paths, true).expect("legacy update");
        let (updated, summary) =
            update_repo_index_runtime(root, &runtime, &paths, true).expect("runtime update");

        assert_eq!(summary.indexed_files, 2);
        assert_eq!(summary.removed_files, 1);
        assert_eq!(updated.materialize_snapshot().as_ref(), Some(&legacy));
    }

    #[test]
    fn rebuild_with_progress_reports_scan_checkpoints() {
        let dir = fixture();
        let mut checkpoints = Vec::new();

        let runtime =
            rebuild_repo_index_runtime_with_progress(dir.path(), true, |indexed, total| {
                checkpoints.push((indexed, total));
            })
            .expect("runtime");

        assert!(runtime.is_loaded());
        assert_eq!(runtime.manifest.total_files, 2);
        assert!(checkpoints
            .iter()
            .any(|&(indexed, total)| indexed == total && total == 2));
    }

    #[test]
    fn clearing_artifacts_preserves_owned_readers() {
        let dir = fixture();
        let runtime = rebuild_repo_index_runtime(dir.path(), true).expect("runtime");

        clear_repo_index_runtime(dir.path()).expect("clear");

        assert!(!repo_index_dir(dir.path()).exists());
        assert!(runtime.file("src/a.rs").is_some());
        let reloaded = load_repo_index_runtime(dir.path()).expect("reload");
        assert!(!reloaded.is_loaded());
        assert_eq!(reloaded.manifest.status, "missing");
    }

    #[test]
    fn repeated_updates_retain_the_base_and_old_reader_view() {
        let dir = fixture();
        let root = dir.path();
        let original = rebuild_repo_index_runtime(root, true).expect("runtime");
        fs::write(root.join("src/a.rs"), "pub fn replacement() {}\n").expect("update");

        let (updated, _) =
            update_repo_index_runtime(root, &original, &[String::from("src/a.rs")], true)
                .expect("incremental update");

        assert!(original.shares_base_with(&updated));
        assert!(original
            .file("src/a.rs")
            .is_some_and(|entry| entry.symbols.iter().any(|symbol| symbol.name == "alpha")));
        assert!(updated.file("src/a.rs").is_some_and(|entry| entry
            .symbols
            .iter()
            .any(|symbol| symbol.name == "replacement")));
    }

    #[test]
    fn threshold_compaction_preserves_parity_and_base_ownership() {
        let dir = fixture();
        let root = dir.path();
        let original = rebuild_repo_index_runtime(root, true).expect("runtime");
        let mut runtime = original.clone();
        let mut legacy = build_repo_index(root, true).expect("legacy");
        let path = String::from("src/a.rs");

        for revision in 0..REPO_INDEX_COMPACTION_SEGMENTS {
            fs::write(
                root.join(&path),
                format!("pub fn revision_{revision}() -> usize {{ {revision} }}\n"),
            )
            .expect("revision");
            update_repo_index(root, &mut legacy, std::slice::from_ref(&path), true)
                .expect("legacy update");
            runtime = update_repo_index_runtime(root, &runtime, std::slice::from_ref(&path), true)
                .expect("runtime update")
                .0;
        }

        assert_eq!(runtime.manifest.segment_count, 1);
        assert!(original.shares_base_with(&runtime));
        assert_eq!(runtime.materialize_snapshot().as_ref(), Some(&legacy));
    }

    #[test]
    fn concurrent_readers_keep_generation_owned_entries() {
        let dir = fixture();
        let root = dir.path();
        let original = rebuild_repo_index_runtime(root, true).expect("runtime");
        fs::write(root.join("src/a.rs"), "pub fn replacement() {}\n").expect("update");
        let updated = update_repo_index_runtime(root, &original, &[String::from("src/a.rs")], true)
            .expect("update")
            .0;
        let barrier = Arc::new(Barrier::new(9));

        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for idx in 0..8 {
                let barrier = Arc::clone(&barrier);
                let runtime = if idx % 2 == 0 {
                    original.clone()
                } else {
                    updated.clone()
                };
                handles.push(scope.spawn(move || {
                    barrier.wait();
                    runtime
                        .file("src/a.rs")
                        .expect("entry")
                        .symbols
                        .first()
                        .expect("symbol")
                        .name
                        .clone()
                }));
            }
            barrier.wait();
            for (idx, handle) in handles.into_iter().enumerate() {
                let expected = if idx % 2 == 0 { "alpha" } else { "replacement" };
                assert_eq!(handle.join().expect("reader"), expected);
            }
        });
    }

    #[test]
    fn corrupt_current_segment_recovers_only_the_retained_previous_generation() {
        let dir = fixture();
        let root = dir.path();
        let base = rebuild_repo_index_runtime(root, true).expect("base");
        fs::write(root.join("src/a.rs"), "pub fn replacement() {}\n").expect("update");
        let updated = update_repo_index_runtime(root, &base, &[String::from("src/a.rs")], true)
            .expect("update")
            .0;
        let record = current_record(root);
        fs::write(
            repo_index_dir(root).join(&record.segment_files[0]),
            b"corrupt segment",
        )
        .expect("corrupt");

        let recovered = load_repo_index_runtime(root).expect("recover");

        assert_eq!(
            recovered.manifest.recovered_from_generation,
            Some(updated.manifest.generation)
        );
        assert_eq!(recovered.manifest.generation, base.manifest.generation);
        assert!(recovered
            .file("src/a.rs")
            .is_some_and(|entry| entry.symbols.iter().any(|symbol| symbol.name == "alpha")));
    }

    #[test]
    fn corrupt_current_base_recovers_a_previous_full_generation() {
        let dir = fixture();
        let root = dir.path();
        let first = rebuild_repo_index_runtime(root, true).expect("first");
        fs::write(root.join("src/c.rs"), "pub struct Charlie;\n").expect("add");
        let second = rebuild_repo_index_runtime(root, true).expect("second");
        let record = current_record(root);
        fs::write(repo_index_dir(root).join(record.base_file), b"corrupt base").expect("corrupt");

        let recovered = load_repo_index_runtime(root).expect("recover");

        assert_eq!(
            recovered.manifest.recovered_from_generation,
            Some(second.manifest.generation)
        );
        assert_eq!(recovered.manifest.generation, first.manifest.generation);
        assert!(recovered.file("src/c.rs").is_none());
    }

    #[test]
    fn retained_previous_base_cannot_be_substituted_for_the_current_generation() {
        let dir = fixture();
        let root = dir.path();
        let first = rebuild_repo_index_runtime(root, true).expect("first");
        fs::write(
            root.join("src/a.rs"),
            "pub fn replacement() -> usize { 1 }\n",
        )
        .expect("replace");
        let second = rebuild_repo_index_runtime(root, true).expect("second");
        assert_eq!(first.manifest.total_files, second.manifest.total_files);
        let previous_record =
            load_generation_record(root, first.manifest.generation).expect("previous record");
        let mut current_record = current_record(root);
        current_record.base_file = previous_record.base_file;
        current_record.base_digest = previous_record.base_digest;
        fs::write(
            repo_index_dir(root).join(generation_record_file_name(current_record.generation)),
            serde_json::to_vec_pretty(&current_record).expect("encode substituted record"),
        )
        .expect("substitute previous base");

        let recovered = load_repo_index_runtime(root).expect("recover");

        assert_eq!(
            recovered.manifest.recovered_from_generation,
            Some(second.manifest.generation)
        );
        assert_eq!(recovered.manifest.generation, first.manifest.generation);
        assert!(recovered.file("src/a.rs").is_some_and(|entry| {
            entry.symbols.iter().any(|symbol| symbol.name == "alpha")
                && entry
                    .symbols
                    .iter()
                    .all(|symbol| symbol.name != "replacement")
        }));
        assert!(recovered
            .manifest
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("generation record")
                && error.contains("failed digest validation")));
    }

    #[test]
    fn corrupt_current_manifest_recovers_the_explicit_backup() {
        let dir = fixture();
        let root = dir.path();
        let base = rebuild_repo_index_runtime(root, true).expect("base");
        fs::write(root.join("src/a.rs"), "pub fn replacement() {}\n").expect("update");
        let _updated = update_repo_index_runtime(root, &base, &[String::from("src/a.rs")], true)
            .expect("update");
        fs::write(manifest_path(root), b"{").expect("corrupt");

        let recovered = load_repo_index_runtime(root).expect("recover");

        assert_eq!(recovered.manifest.generation, base.manifest.generation);
        assert!(recovered.manifest.last_error.is_some());
    }

    #[test]
    fn unpublished_orphan_generation_is_never_promoted() {
        let dir = fixture();
        let root = dir.path();
        let runtime = rebuild_repo_index_runtime(root, true).expect("runtime");
        let orphan_generation = runtime.manifest.generation + 1;
        fs::write(
            repo_index_dir(root).join(generation_record_file_name(orphan_generation)),
            b"{\"schema_version\":1}",
        )
        .expect("orphan");
        fs::write(
            repo_index_dir(root).join(segment_file_name(orphan_generation)),
            b"partial",
        )
        .expect("partial segment");

        let loaded = load_repo_index_runtime(root).expect("load");

        assert_eq!(loaded.manifest.generation, runtime.manifest.generation);
        assert!(loaded.is_loaded());
    }

    #[test]
    fn generation_record_count_mismatch_recovers_previous_generation() {
        let dir = fixture();
        let root = dir.path();
        let base = rebuild_repo_index_runtime(root, true).expect("base");
        fs::write(root.join("src/a.rs"), "pub fn replacement() {}\n").expect("update");
        let updated = update_repo_index_runtime(root, &base, &[String::from("src/a.rs")], true)
            .expect("update")
            .0;
        let mut record = current_record(root);
        record.manifest.total_files += 1;
        fs::write(
            repo_index_dir(root).join(generation_record_file_name(record.generation)),
            serde_json::to_vec_pretty(&record).expect("encode"),
        )
        .expect("rewrite");

        let recovered = load_repo_index_runtime(root).expect("recover");

        assert_eq!(
            recovered.manifest.recovered_from_generation,
            Some(updated.manifest.generation)
        );
        assert_eq!(recovered.manifest.generation, base.manifest.generation);
    }

    #[test]
    fn segment_validation_rejects_path_mismatch_and_entry_tombstone_overlap() {
        let mut segment = RepoIndexSegment {
            version: REPO_INDEX_SEGMENT_VERSION,
            generation: 1,
            ..RepoIndexSegment::default()
        };
        segment.files.insert(
            "src/key.rs".to_string(),
            RepoIndexFileEntry {
                path: "src/value.rs".to_string(),
                ..RepoIndexFileEntry::default()
            },
        );
        assert!(validate_segment(&segment)
            .expect_err("path mismatch")
            .to_string()
            .contains("does not match"));

        let entry = segment.files.remove("src/key.rs").expect("entry");
        segment.files.insert(entry.path.clone(), entry);
        segment.tombstones.insert("src/value.rs".to_string());
        assert!(validate_segment(&segment)
            .expect_err("overlap")
            .to_string()
            .contains("both an entry and tombstone"));
    }

    #[test]
    fn changed_paths_cannot_escape_the_repository_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("workspace");
        fs::create_dir_all(root.join("src")).expect("workspace");
        fs::write(root.join("src/lib.rs"), "pub fn inside() {}\n").expect("inside");
        fs::write(dir.path().join("outside.rs"), "pub fn outside() {}\n").expect("outside");
        let runtime = rebuild_repo_index_runtime(&root, true).expect("runtime");
        let mut legacy = build_repo_index(&root, true).expect("legacy");

        let runtime_error =
            update_repo_index_runtime(&root, &runtime, &[String::from("../outside.rs")], true)
                .expect_err("runtime traversal");
        let legacy_error =
            update_repo_index(&root, &mut legacy, &[String::from("../outside.rs")], true)
                .expect_err("legacy traversal");

        assert!(runtime_error.to_string().contains("beneath"));
        assert!(legacy_error.to_string().contains("beneath"));
        assert_eq!(
            load_repo_index_runtime(&root)
                .expect("reload")
                .manifest
                .generation,
            runtime.manifest.generation
        );
    }

    #[test]
    fn concurrent_writers_return_an_explicit_generation_conflict() {
        let dir = fixture();
        let root = dir.path();
        let base = rebuild_repo_index_runtime(root, true).expect("base");
        fs::write(root.join("src/c.rs"), "pub struct Charlie;\n").expect("c");
        fs::write(root.join("src/d.rs"), "pub struct Delta;\n").expect("d");
        let barrier = Arc::new(Barrier::new(3));

        let results = std::thread::scope(|scope| {
            let handles = ["src/c.rs", "src/d.rs"].map(|path| {
                let runtime = base.clone();
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    barrier.wait();
                    update_repo_index_runtime(root, &runtime, &[path.to_string()], true)
                })
            });
            barrier.wait();
            handles.map(|handle| handle.join().expect("writer"))
        });

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let conflict = results
            .iter()
            .find_map(|result| result.as_ref().err())
            .expect("conflict");
        assert!(conflict.to_string().contains("generation conflict"));
        let loaded = load_repo_index_runtime(root).expect("load winner");
        assert!(loaded.is_loaded());
        assert_eq!(loaded.manifest.generation, base.manifest.generation + 1);
        assert_eq!(
            usize::from(loaded.file("src/c.rs").is_some())
                + usize::from(loaded.file("src/d.rs").is_some()),
            1
        );
    }

    #[test]
    fn structurally_valid_segment_mutation_recovers_previous_generation() {
        let dir = fixture();
        let root = dir.path();
        let base = rebuild_repo_index_runtime(root, true).expect("base");
        fs::write(root.join("src/a.rs"), "pub fn replacement() {}\n").expect("update");
        let updated = update_repo_index_runtime(root, &base, &[String::from("src/a.rs")], true)
            .expect("update")
            .0;
        let record = current_record(root);
        let segment_path = repo_index_dir(root).join(&record.segment_files[0]);
        let raw = fs::read(&segment_path).expect("segment");
        let mut segment = wincode::deserialize::<RepoIndexSegment>(&raw).expect("decode");
        segment.files.get_mut("src/a.rs").expect("entry").size += 1;
        fs::write(
            &segment_path,
            wincode::serialize(&segment).expect("valid segment"),
        )
        .expect("mutate");

        let recovered = load_repo_index_runtime(root).expect("recover");

        assert_eq!(
            recovered.manifest.recovered_from_generation,
            Some(updated.manifest.generation)
        );
        assert_eq!(recovered.manifest.generation, base.manifest.generation);
        assert!(recovered
            .manifest
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("digest validation")));
    }

    #[test]
    fn retention_keeps_only_current_and_previous_full_generations() {
        let dir = fixture();
        let root = dir.path();
        let first = rebuild_repo_index_runtime(root, true).expect("first");
        fs::write(root.join("src/c.rs"), "pub struct Charlie;\n").expect("c");
        let second = rebuild_repo_index_runtime(root, true).expect("second");
        fs::write(root.join("src/d.rs"), "pub struct Delta;\n").expect("d");
        let third = rebuild_repo_index_runtime(root, true).expect("third");
        let names = fs::read_dir(repo_index_dir(root))
            .expect("index directory")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .to_string()
            })
            .collect::<Vec<_>>();

        assert_eq!(
            names
                .iter()
                .filter(|name| name.starts_with("generation-"))
                .count(),
            2
        );
        assert_eq!(
            names
                .iter()
                .filter(|name| name.starts_with("base-"))
                .count(),
            2
        );
        assert!(!names.contains(&generation_record_file_name(first.manifest.generation)));
        assert!(names.contains(&generation_record_file_name(second.manifest.generation)));
        assert!(names.contains(&generation_record_file_name(third.manifest.generation)));
    }
}
