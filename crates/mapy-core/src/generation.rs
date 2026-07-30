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
//! same-directory rename: generation identities are persistently reserved
//! before create-once artifacts are written, and the manifest is renamed last.
//! Files and parent directories are not `fsync`ed, so this module does not claim
//! power-loss durability.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use fs2::FileExt;
use packet28_state_fs::{FileAccess, StateDir, StateFile};
use serde::{Deserialize, Serialize};
use suite_packet_core::CovyError;

use crate::runtime::{index_repo_path, normalize_path, MAP_CACHE_VERSION};
use crate::{
    build_repo_index_with_progress, RepoIndexFileEntry, RepoIndexSnapshot, RepoIndexUpdateSummary,
    RepoIndexUpdateWork,
};

const REPO_INDEX_RUNTIME_SCHEMA_VERSION: u32 = 1;
const REPO_INDEX_GENERATION_HIGH_WATER_SCHEMA_VERSION: u32 = 1;
const REPO_INDEX_DIR_NAME: &str = "mapy-v1";
const REPO_INDEX_MANIFEST_FILE: &str = "manifest.json";
const REPO_INDEX_PREVIOUS_MANIFEST_FILE: &str = "manifest.previous.json";
const REPO_INDEX_WRITER_LOCK_FILE: &str = ".mapy-v1.writer.lock";
const REPO_INDEX_GENERATION_HIGH_WATER_FILE: &str = ".mapy-v1.generation-high-water.json";
const REPO_INDEX_COMPACTION_SEGMENTS: usize = 8;
const REPO_INDEX_SEGMENT_VERSION: u32 = 1;
const MAX_REPO_INDEX_METADATA_BYTES: u64 = 8 * 1024 * 1024;
const MAX_REPO_INDEX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_GENERATION_RECORD_BYTES: u64 = 64 * 1024;

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
        let generation = self.generation.as_ref()?;
        let mut files = BTreeMap::new();
        self.for_each_file(|entry| {
            files.insert(entry.path.clone(), entry.clone());
        });
        Some(RepoIndexSnapshot {
            version: MAP_CACHE_VERSION,
            include_tests: generation.base.include_tests,
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
    previous_record: Option<RepoIndexGenerationRecord>,
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
        let _ = prune_generation_artifacts(&self.root, &self.record, self.previous_record.as_ref());
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

#[derive(Debug, Clone)]
struct RepoIndexGeneration {
    publication_identity: PublicationIdentity,
    base: Arc<RepoIndexSnapshot>,
    base_file: String,
    base_digest: String,
    base_stamp: ArtifactStamp,
    segments: Vec<Arc<RepoIndexSegment>>,
    segment_files: Vec<String>,
    segment_digests: Vec<String>,
    segment_stamps: Vec<ArtifactStamp>,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RepoIndexGenerationHighWater {
    schema_version: u32,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PublicationIdentity {
    generation: u64,
    generation_record_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArtifactStamp {
    len: u64,
    modified_unix_nanos: Option<i128>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed_unix_nanos: i128,
}

struct ValidatedArtifactStamps {
    base: ArtifactStamp,
    segments: Vec<ArtifactStamp>,
    pins: Vec<PinnedArtifact>,
    metadata_checks: usize,
    bytes_hashed: usize,
}

struct PinnedArtifact {
    path: PathBuf,
    file: StateFile,
    stamp: ArtifactStamp,
    expected_digest: String,
}

struct PinnedGenerationRecord {
    root: PathBuf,
    path: PathBuf,
    file: StateFile,
    stamp: ArtifactStamp,
    expected_manifest: RepoIndexRuntimeManifest,
}

struct AuthenticatedRecovery {
    manifest: RepoIndexRuntimeManifest,
    record: RepoIndexGenerationRecord,
}

#[derive(Default)]
struct PublicationFenceWork {
    publication_metadata_bytes_decoded: usize,
    repository_artifact_bytes_hashed: usize,
    repository_artifact_metadata_checks: usize,
}

struct IncrementalPublication<'a> {
    root: &'a Path,
    previous: &'a RepoIndexRuntimeManifest,
    current: &'a RepoIndexRuntimeManifest,
    record_pins: &'a [PinnedGenerationRecord],
    artifact_pins: &'a [PinnedArtifact],
    previous_current_bytes: Option<&'a [u8]>,
    previous_previous_bytes: Option<&'a [u8]>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IncrementalPublicationStage {
    BeforeCurrent,
    AfterCurrent,
    AfterPrevious,
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
    let previous = load_authenticated_recovery(root)?;
    let generation = reserve_generation(root)?;
    let base_file = base_file_name(generation);
    let base_bytes = wincode::serialize(&snapshot)
        .map_err(|error| cache_error(format!("failed to encode repository base: {error}")))?;
    write_immutable(repo_index_dir(root).join(&base_file), &base_bytes)?;
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
    publish_manifest(
        root,
        previous.as_ref().map(|recovery| &recovery.manifest),
        &manifest,
    )?;
    let _ = prune_generation_artifacts(
        root,
        &record,
        previous.as_ref().map(|recovery| &recovery.record),
    );
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
    let previous = load_authenticated_recovery(root)?;
    let generation = reserve_generation(root)?;
    let base_file = base_file_name(generation);
    let base_bytes = wincode::serialize(&snapshot)
        .map_err(|error| cache_error(format!("failed to encode repository base: {error}")))?;
    write_immutable(repo_index_dir(root).join(&base_file), &base_bytes)?;
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
        previous: previous.as_ref().map(|recovery| recovery.manifest.clone()),
        previous_record: previous.map(|recovery| recovery.record),
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
/// immutable generation data independently of the on-disk artifacts. Durable
/// generation fencing remains beside the writer lock so a later rebuild cannot
/// reuse an identity owned by one of those handles.
///
/// # Errors
///
/// Returns [`CovyError::Cache`] when the generation directory cannot be
/// removed.
pub fn clear_repo_index_runtime(root: &Path) -> Result<(), CovyError> {
    let _writer = acquire_writer_lock(root)?;
    ensure_generation_high_water(root)?;
    let path = repo_index_dir(root);
    let parent = StateDir::open(root, &[".packet28", "index"], false).map_err(|error| {
        cache_error(format!(
            "failed to open repository index parent '{}': {error}",
            repo_index_parent(root).display()
        ))
    })?;
    parent
        .remove_tree_if_exists(REPO_INDEX_DIR_NAME)
        .map_err(|error| {
            cache_error(format!(
                "failed to remove repository index directory '{}': {error}",
                path.display()
            ))
        })?;
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
    update_repo_index_runtime_with_hook(root, current, changed_paths, include_tests, |_| Ok(()))
}

fn update_repo_index_runtime_with_hook<F>(
    root: &Path,
    current: &RepoIndexRuntime,
    changed_paths: &[String],
    include_tests: bool,
    mut publication_hook: F,
) -> Result<(RepoIndexRuntime, RepoIndexUpdateSummary), CovyError>
where
    F: FnMut(IncrementalPublicationStage) -> Result<(), CovyError>,
{
    let loaded = current
        .generation
        .as_ref()
        .ok_or_else(|| cache_error("repository index runtime is not loaded"))?;
    let policy_changed = loaded.base.include_tests != include_tests;
    if changed_paths.is_empty() && !policy_changed {
        return Ok((
            current.clone(),
            RepoIndexUpdateSummary {
                indexed_files: 0,
                removed_files: 0,
                changed_paths: Vec::new(),
                work: RepoIndexUpdateWork::default(),
            },
        ));
    }
    let normalized = normalize_changed_paths(root, changed_paths)?;
    let changed_paths_considered = normalized.len();
    let _writer = acquire_writer_lock(root)?;
    let (published, previous_current_bytes) = load_published_manifest_with_bytes(root)?;
    let manifest_bytes_decoded = previous_current_bytes.as_ref().map_or(0, Vec::len);
    let previous_previous_bytes = read_optional_file(&previous_manifest_path(root))?;
    let published_identity = publication_identity(&published)?;
    let current_identity = &loaded.publication_identity;
    if &published_identity != current_identity {
        return Err(cache_error(format!(
            "repository index generation conflict: caller has {}@{}, published generation is {}@{}",
            current_identity.generation,
            current_identity.generation_record_digest,
            published_identity.generation,
            published_identity.generation_record_digest
        )));
    }
    let (published_record, published_record_pin, generation_record_bytes_decoded) =
        pin_authenticated_generation_record(root, &published)?;
    let mut publication_metadata_bytes_decoded =
        manifest_bytes_decoded.saturating_add(generation_record_bytes_decoded);
    let validated_artifacts = validate_loaded_artifact_stamps(root, loaded)?;
    let mut repository_artifact_metadata_checks = validated_artifacts.metadata_checks;
    let mut repository_artifact_bytes_hashed = validated_artifacts.bytes_hashed;
    if policy_changed {
        let snapshot = build_repo_index_with_progress(root, include_tests, |_, _| {})?;
        let rebuilt = publish_rebuilt_runtime(root, include_tests, snapshot)?;
        return Ok((
            rebuilt,
            RepoIndexUpdateSummary {
                indexed_files: 0,
                removed_files: 0,
                changed_paths: normalized,
                work: RepoIndexUpdateWork {
                    publication_metadata_bytes_decoded,
                    repository_artifact_bytes_hashed,
                    repository_artifact_metadata_checks,
                    changed_paths_considered,
                    ..RepoIndexUpdateWork::default()
                },
            },
        ));
    }
    let ValidatedArtifactStamps {
        base: validated_base_stamp,
        segments: mut segment_stamps,
        pins: mut artifact_pins,
        ..
    } = validated_artifacts;
    let generation = reserve_generation(root)?;
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
    let (segment, segment_digest, segment_stamp) = persist_segment(root, &segment_file, &segment)?;
    let segment = Arc::new(segment);
    segment_files.push(segment_file);
    segment_digests.push(segment_digest);
    segment_stamps.push(segment_stamp);

    let mut latest_by_path = loaded.latest_by_path.clone();
    apply_segment_resolution(&segment, &mut latest_by_path);
    segments.push(segment);

    if segments.len() >= REPO_INDEX_COMPACTION_SEGMENTS {
        let compacted = compact_segments(generation, &segments, &latest_by_path);
        let compacted_file = compacted_segment_file_name(generation);
        let (compacted, compacted_digest, compacted_stamp) =
            persist_segment(root, &compacted_file, &compacted)?;
        let compacted = Arc::new(compacted);
        latest_by_path.clear();
        apply_segment_resolution(&compacted, &mut latest_by_path);
        segments = vec![compacted];
        segment_files = vec![compacted_file];
        segment_digests = vec![compacted_digest];
        segment_stamps = vec![compacted_stamp];
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
    let (persisted_record, next_record_pin, next_record_bytes_decoded) =
        pin_authenticated_generation_record(root, &manifest)?;
    publication_metadata_bytes_decoded =
        publication_metadata_bytes_decoded.saturating_add(next_record_bytes_decoded);
    record = persisted_record;

    let previously_pinned = loaded
        .segment_files
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    for ((file_name, expected_digest), stamp) in segment_files
        .iter()
        .zip(&segment_digests)
        .zip(&mut segment_stamps)
    {
        if previously_pinned.contains(file_name.as_str()) {
            continue;
        }
        let (authenticated_stamp, pin) = pin_loaded_artifact(
            &repo_index_dir(root).join(file_name),
            stamp,
            expected_digest,
            &mut repository_artifact_metadata_checks,
            &mut repository_artifact_bytes_hashed,
        )?;
        *stamp = authenticated_stamp;
        artifact_pins.push(pin);
    }

    let runtime = RepoIndexRuntime {
        manifest: manifest.clone(),
        generation: Some(Arc::new(RepoIndexGeneration {
            publication_identity: publication_identity(&manifest)?,
            base: Arc::clone(&loaded.base),
            base_file: loaded.base_file.clone(),
            base_digest: loaded.base_digest.clone(),
            base_stamp: validated_base_stamp,
            segments,
            segment_files,
            segment_digests,
            segment_stamps,
            latest_by_path,
        })),
    };
    validate_runtime(&runtime)?;
    let record_pins = [published_record_pin, next_record_pin];
    let fence_work = publish_incremental_manifests(
        IncrementalPublication {
            root,
            previous: &published,
            current: &manifest,
            record_pins: &record_pins,
            artifact_pins: &artifact_pins,
            previous_current_bytes: previous_current_bytes.as_deref(),
            previous_previous_bytes: previous_previous_bytes.as_deref(),
        },
        &mut publication_hook,
    )?;
    publication_metadata_bytes_decoded = publication_metadata_bytes_decoded
        .saturating_add(fence_work.publication_metadata_bytes_decoded);
    repository_artifact_bytes_hashed = repository_artifact_bytes_hashed
        .saturating_add(fence_work.repository_artifact_bytes_hashed);
    repository_artifact_metadata_checks = repository_artifact_metadata_checks
        .saturating_add(fence_work.repository_artifact_metadata_checks);
    let _ = prune_generation_artifacts(root, &record, Some(&published_record));
    Ok((
        runtime,
        RepoIndexUpdateSummary {
            indexed_files,
            removed_files,
            changed_paths: normalized,
            work: RepoIndexUpdateWork {
                publication_metadata_bytes_decoded,
                repository_artifact_bytes_hashed,
                repository_artifact_metadata_checks,
                changed_paths_considered,
                ..RepoIndexUpdateWork::default()
            },
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
        let current_exists =
            read_optional_state_file(&manifest_path(root), MAX_REPO_INDEX_METADATA_BYTES)?
                .is_some();
        let previous_exists =
            read_optional_state_file(&previous_manifest_path(root), MAX_REPO_INDEX_METADATA_BYTES)?
                .is_some();
        let recovered = recover_previous(
            root,
            None,
            cache_error("repository index current manifest is missing or schema-zero"),
        )?;
        if recovered.is_loaded() || current_exists || previous_exists {
            return Ok(recovered);
        }
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
    let record = load_authenticated_generation_record(root, expected_manifest)?;
    validate_artifact_name(&record.base_file)?;
    let base_path = repo_index_dir(root).join(&record.base_file);
    let (base_raw, base_stamp) = read_authenticated_artifact(&base_path, &record.base_digest)?;
    let base = wincode::deserialize::<RepoIndexSnapshot>(&base_raw).map_err(|error| {
        cache_error(format!(
            "failed to decode repository base '{}': {error}",
            base_path.display()
        ))
    })?;
    validate_base(&base, expected_manifest.include_tests)?;

    let record_path =
        repo_index_dir(root).join(generation_record_file_name(expected_manifest.generation));
    let mut segments = Vec::with_capacity(record.segment_files.len());
    let mut segment_stamps = Vec::with_capacity(record.segment_files.len());
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
        let (raw, stamp) = read_authenticated_artifact(&path, expected_digest)?;
        let segment = wincode::deserialize::<RepoIndexSegment>(&raw).map_err(|error| {
            cache_error(format!(
                "failed to decode repository segment '{}': {error}",
                path.display()
            ))
        })?;
        validate_segment(&segment)?;
        segment_stamps.push(stamp);
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
        publication_identity: publication_identity(expected_manifest)?,
        base: Arc::new(base),
        base_file: record.base_file,
        base_digest: record.base_digest,
        base_stamp,
        segments,
        segment_files: record.segment_files,
        segment_digests: record.segment_digests,
        segment_stamps,
        latest_by_path,
    };
    let runtime = RepoIndexRuntime {
        manifest: expected_manifest.clone(),
        generation: Some(Arc::new(generation)),
    };
    validate_runtime(&runtime)?;
    Ok(runtime)
}

fn load_authenticated_generation_record(
    root: &Path,
    expected_manifest: &RepoIndexRuntimeManifest,
) -> Result<RepoIndexGenerationRecord, CovyError> {
    load_authenticated_generation_record_with_decoded_bytes(root, expected_manifest)
        .map(|(record, _)| record)
}

fn load_authenticated_generation_record_with_decoded_bytes(
    root: &Path,
    expected_manifest: &RepoIndexRuntimeManifest,
) -> Result<(RepoIndexGenerationRecord, usize), CovyError> {
    pin_authenticated_generation_record(root, expected_manifest)
        .map(|(record, _, decoded_bytes)| (record, decoded_bytes))
}

fn pin_authenticated_generation_record(
    root: &Path,
    expected_manifest: &RepoIndexRuntimeManifest,
) -> Result<(RepoIndexGenerationRecord, PinnedGenerationRecord, usize), CovyError> {
    validate_manifest(expected_manifest)?;
    let record_path =
        repo_index_dir(root).join(generation_record_file_name(expected_manifest.generation));
    let (directory, name) = repo_state_location(&record_path, false)?;
    let Some(mut file) = directory
        .open_existing(&name, FileAccess::ReadOnly)
        .map_err(|error| {
            cache_error(format!(
                "failed to open generation record '{}': {error}",
                record_path.display()
            ))
        })?
    else {
        return Err(cache_error(format!(
            "generation record '{}' does not exist",
            record_path.display()
        )));
    };
    let before_metadata = file.file().metadata().map_err(|error| {
        cache_error(format!(
            "failed to inspect generation record '{}': {error}",
            record_path.display()
        ))
    })?;
    ensure_regular_artifact(&record_path, &before_metadata)?;
    if before_metadata.len() > MAX_GENERATION_RECORD_BYTES {
        return Err(cache_error(format!(
            "generation record '{}' is {} bytes; maximum is {MAX_GENERATION_RECORD_BYTES}",
            record_path.display(),
            before_metadata.len()
        )));
    }
    let before = artifact_stamp_from_metadata(&before_metadata);
    let capacity = usize::try_from(before_metadata.len()).map_err(|_| {
        cache_error(format!(
            "generation record '{}' exceeds the platform allocation limit",
            record_path.display()
        ))
    })?;
    let mut raw = Vec::new();
    raw.try_reserve_exact(capacity).map_err(|error| {
        cache_error(format!(
            "failed to reserve generation record '{}': {error}",
            record_path.display()
        ))
    })?;
    file.file_mut().seek(SeekFrom::Start(0)).map_err(|error| {
        cache_error(format!(
            "failed to seek generation record '{}': {error}",
            record_path.display()
        ))
    })?;
    Read::by_ref(file.file_mut())
        .take(MAX_GENERATION_RECORD_BYTES.saturating_add(1))
        .read_to_end(&mut raw)
        .map_err(|error| {
            cache_error(format!(
                "failed to read generation record '{}': {error}",
                record_path.display()
            ))
        })?;
    if u64::try_from(raw.len()).unwrap_or(u64::MAX) > MAX_GENERATION_RECORD_BYTES {
        return Err(cache_error(format!(
            "generation record '{}' exceeds the {MAX_GENERATION_RECORD_BYTES}-byte limit",
            record_path.display()
        )));
    }
    let after_metadata = file.file().metadata().map_err(|error| {
        cache_error(format!(
            "failed to re-inspect generation record '{}': {error}",
            record_path.display()
        ))
    })?;
    ensure_regular_artifact(&record_path, &after_metadata)?;
    let after = artifact_stamp_from_metadata(&after_metadata);
    if before != after {
        return Err(cache_error(format!(
            "generation record '{}' changed while it was being authenticated",
            record_path.display()
        )));
    }
    file.validate_attachment().map_err(|error| {
        cache_error(format!(
            "generation record '{}' was replaced while it was being authenticated: {error}",
            record_path.display()
        ))
    })?;
    let record = authenticate_generation_record_bytes(&record_path, &raw, expected_manifest)?;
    let decoded_bytes = raw.len();
    Ok((
        record,
        PinnedGenerationRecord {
            root: root.to_path_buf(),
            path: record_path,
            file,
            stamp: after,
            expected_manifest: expected_manifest.clone(),
        },
        decoded_bytes,
    ))
}

fn authenticate_generation_record_bytes(
    record_path: &Path,
    raw: &[u8],
    expected_manifest: &RepoIndexRuntimeManifest,
) -> Result<RepoIndexGenerationRecord, CovyError> {
    let record = serde_json::from_slice::<RepoIndexGenerationRecord>(raw).map_err(|error| {
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
    let persisted_encoding = serde_json::to_vec_pretty(&record).map_err(|error| {
        cache_error(format!(
            "failed to encode generation record '{}': {error}",
            record_path.display()
        ))
    })?;
    if raw != persisted_encoding {
        return Err(cache_error(format!(
            "generation record '{}' is not in its authenticated canonical persisted encoding",
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
    Ok(record)
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
    if loaded.base.include_tests != runtime.manifest.include_tests {
        return Err(cache_error(
            "repository manifest include-tests policy does not match its loaded base",
        ));
    }
    if loaded.segments.len() != loaded.segment_files.len()
        || loaded.segments.len() != loaded.segment_digests.len()
        || loaded.segments.len() != loaded.segment_stamps.len()
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
) -> Result<(RepoIndexSegment, String, ArtifactStamp), CovyError> {
    let encoded = wincode::serialize(segment)
        .map_err(|error| cache_error(format!("failed to encode repository segment: {error}")))?;
    let path = repo_index_dir(root).join(file_name);
    write_immutable(path.clone(), &encoded)?;
    let digest = artifact_digest(&encoded);
    let (persisted, stamp) = read_authenticated_artifact(&path, &digest)?;
    let decoded = wincode::deserialize::<RepoIndexSegment>(&persisted)
        .map_err(|error| cache_error(format!("failed to validate repository segment: {error}")))?;
    validate_segment(&decoded)?;
    Ok((decoded, digest, stamp))
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
    write_immutable(
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

fn publication_identity(
    manifest: &RepoIndexRuntimeManifest,
) -> Result<PublicationIdentity, CovyError> {
    validate_manifest(manifest)?;
    let generation_record_digest = manifest.generation_record_digest.clone().ok_or_else(|| {
        cache_error(format!(
            "published repository generation {} does not authenticate its generation record",
            manifest.generation
        ))
    })?;
    Ok(PublicationIdentity {
        generation: manifest.generation,
        generation_record_digest,
    })
}

fn publish_manifest(
    root: &Path,
    previous: Option<&RepoIndexRuntimeManifest>,
    current: &RepoIndexRuntimeManifest,
) -> Result<(), CovyError> {
    if let Some(previous) = previous.filter(|manifest| manifest.generation > 0) {
        publish_previous_manifest(root, previous)?;
    }
    publish_current_manifest(root, current)
}

fn publish_current_manifest(
    root: &Path,
    current: &RepoIndexRuntimeManifest,
) -> Result<(), CovyError> {
    let encoded = serde_json::to_vec_pretty(current)
        .map_err(|error| cache_error(format!("failed to encode repository manifest: {error}")))?;
    write_atomic(manifest_path(root), &encoded)
}

fn publish_previous_manifest(
    root: &Path,
    previous: &RepoIndexRuntimeManifest,
) -> Result<(), CovyError> {
    let encoded = serde_json::to_vec_pretty(&durable_manifest(previous)).map_err(|error| {
        cache_error(format!(
            "failed to encode previous repository manifest: {error}"
        ))
    })?;
    write_atomic(previous_manifest_path(root), &encoded)
}

fn publish_incremental_manifests<F>(
    publication: IncrementalPublication<'_>,
    publication_hook: &mut F,
) -> Result<PublicationFenceWork, CovyError>
where
    F: FnMut(IncrementalPublicationStage) -> Result<(), CovyError>,
{
    let result = (|| {
        publication_hook(IncrementalPublicationStage::BeforeCurrent)?;
        publish_current_manifest(publication.root, publication.current)?;
        publication_hook(IncrementalPublicationStage::AfterCurrent)?;
        let mut work =
            revalidate_publication_fence(publication.record_pins, publication.artifact_pins)?;
        publish_previous_manifest(publication.root, publication.previous)?;
        publication_hook(IncrementalPublicationStage::AfterPrevious)?;
        let final_work =
            revalidate_publication_fence(publication.record_pins, publication.artifact_pins)?;
        work.publication_metadata_bytes_decoded = work
            .publication_metadata_bytes_decoded
            .saturating_add(final_work.publication_metadata_bytes_decoded);
        work.repository_artifact_bytes_hashed = work
            .repository_artifact_bytes_hashed
            .saturating_add(final_work.repository_artifact_bytes_hashed);
        work.repository_artifact_metadata_checks = work
            .repository_artifact_metadata_checks
            .saturating_add(final_work.repository_artifact_metadata_checks);
        Ok(work)
    })();
    match result {
        Ok(work) => Ok(work),
        Err(error) => Err(rollback_incremental_manifests(
            publication.root,
            publication.previous_current_bytes,
            publication.previous_previous_bytes,
            error,
        )),
    }
}

fn revalidate_publication_fence(
    record_pins: &[PinnedGenerationRecord],
    artifact_pins: &[PinnedArtifact],
) -> Result<PublicationFenceWork, CovyError> {
    let mut work = PublicationFenceWork::default();
    for pin in record_pins {
        revalidate_pinned_leaf(&pin.path, &pin.file, &pin.stamp, "generation record")?;
        if !cfg!(unix) {
            let (_, decoded_bytes) = load_authenticated_generation_record_with_decoded_bytes(
                &pin.root,
                &pin.expected_manifest,
            )?;
            work.publication_metadata_bytes_decoded = work
                .publication_metadata_bytes_decoded
                .saturating_add(decoded_bytes);
        }
    }
    for pin in artifact_pins {
        revalidate_pinned_leaf(&pin.path, &pin.file, &pin.stamp, "artifact")?;
        work.repository_artifact_metadata_checks =
            work.repository_artifact_metadata_checks.saturating_add(1);
        if !cfg!(unix) {
            let (raw, _) = read_authenticated_artifact(&pin.path, &pin.expected_digest)?;
            work.repository_artifact_bytes_hashed = work
                .repository_artifact_bytes_hashed
                .saturating_add(raw.len());
        }
    }
    Ok(work)
}

fn revalidate_pinned_leaf(
    path: &Path,
    file: &StateFile,
    expected_stamp: &ArtifactStamp,
    kind: &str,
) -> Result<(), CovyError> {
    let metadata = file.file().metadata().map_err(|error| {
        cache_error(format!(
            "failed to inspect pinned repository index {kind} '{}': {error}",
            path.display()
        ))
    })?;
    ensure_regular_artifact(path, &metadata)?;
    let descriptor_stamp = artifact_stamp_from_metadata(&metadata);
    file.validate_attachment().map_err(|error| {
        cache_error(format!(
            "pinned repository index {kind} '{}' was replaced during publication: {error}",
            path.display()
        ))
    })?;
    if descriptor_stamp != *expected_stamp {
        return Err(cache_error(format!(
            "repository index {kind} '{}' changed during publication",
            path.display()
        )));
    }
    Ok(())
}

fn rollback_incremental_manifests(
    root: &Path,
    previous_current_bytes: Option<&[u8]>,
    previous_previous_bytes: Option<&[u8]>,
    publication_error: CovyError,
) -> CovyError {
    let previous_restore =
        restore_optional_file(previous_manifest_path(root), previous_previous_bytes);
    let current_restore = restore_optional_file(manifest_path(root), previous_current_bytes);
    match (previous_restore, current_restore) {
        (Ok(()), Ok(())) => publication_error,
        (previous, current) => cache_error(format!(
            "incremental repository publication failed ({publication_error}); restoring previous manifest: {}; restoring current manifest: {}",
            previous
                .err()
                .map_or_else(|| "ok".to_string(), |error| error.to_string()),
            current
                .err()
                .map_or_else(|| "ok".to_string(), |error| error.to_string())
        )),
    }
}

fn durable_manifest(manifest: &RepoIndexRuntimeManifest) -> RepoIndexRuntimeManifest {
    let mut durable = manifest.clone();
    durable.recovered_from_generation = None;
    durable.last_error = None;
    durable
}

fn load_authenticated_recovery(root: &Path) -> Result<Option<AuthenticatedRecovery>, CovyError> {
    let runtime = load_repo_index_runtime(root)?;
    if !runtime.is_loaded() {
        return Ok(None);
    }
    let manifest = durable_manifest(&runtime.manifest);
    let record = load_authenticated_generation_record(root, &manifest)?;
    Ok(Some(AuthenticatedRecovery { manifest, record }))
}

fn load_published_manifest(root: &Path) -> Result<RepoIndexRuntimeManifest, CovyError> {
    load_published_manifest_with_decoded_bytes(root).map(|(manifest, _)| manifest)
}

fn load_published_manifest_with_decoded_bytes(
    root: &Path,
) -> Result<(RepoIndexRuntimeManifest, usize), CovyError> {
    load_published_manifest_with_bytes(root)
        .map(|(manifest, bytes)| (manifest, bytes.as_ref().map_or(0, Vec::len)))
}

fn load_published_manifest_with_bytes(
    root: &Path,
) -> Result<(RepoIndexRuntimeManifest, Option<Vec<u8>>), CovyError> {
    let path = manifest_path(root);
    let Some(raw) = read_optional_state_file(&path, MAX_REPO_INDEX_METADATA_BYTES)? else {
        return Ok((RepoIndexRuntimeManifest::default(), None));
    };
    let manifest = decode_manifest_path(&path, &raw)?;
    Ok((manifest, Some(raw)))
}

fn load_previous_manifest(root: &Path) -> Result<RepoIndexRuntimeManifest, CovyError> {
    load_manifest_path(&previous_manifest_path(root))
}

fn load_manifest_path(path: &Path) -> Result<RepoIndexRuntimeManifest, CovyError> {
    load_manifest_path_with_decoded_bytes(path).map(|(manifest, _)| manifest)
}

fn load_manifest_path_with_decoded_bytes(
    path: &Path,
) -> Result<(RepoIndexRuntimeManifest, usize), CovyError> {
    let raw = read_state_file(path, MAX_REPO_INDEX_METADATA_BYTES)?;
    let decoded_bytes = raw.len();
    let manifest = decode_manifest_path(path, &raw)?;
    Ok((manifest, decoded_bytes))
}

fn decode_manifest_path(path: &Path, raw: &[u8]) -> Result<RepoIndexRuntimeManifest, CovyError> {
    serde_json::from_slice(raw).map_err(|error| {
        cache_error(format!(
            "failed to decode repository manifest '{}': {error}",
            path.display()
        ))
    })
}

fn reserve_generation(root: &Path) -> Result<u64, CovyError> {
    let observed = discover_generation_high_water(root)?;
    let current = match load_generation_high_water(root)? {
        Some(stored) if stored < observed => {
            return Err(cache_error(format!(
                "repository generation high-water {stored} trails observed generation {observed}"
            )));
        }
        Some(stored) => stored,
        None => observed,
    };
    let next = current
        .checked_add(1)
        .ok_or_else(|| cache_error("repository generation high-water is exhausted at u64::MAX"))?;
    persist_generation_high_water(root, next)?;
    Ok(next)
}

fn ensure_generation_high_water(root: &Path) -> Result<u64, CovyError> {
    let observed = discover_generation_high_water(root)?;
    match load_generation_high_water(root)? {
        Some(stored) if stored < observed => Err(cache_error(format!(
            "repository generation high-water {stored} trails observed generation {observed}"
        ))),
        Some(stored) => Ok(stored),
        None => {
            persist_generation_high_water(root, observed)?;
            Ok(observed)
        }
    }
}

fn load_generation_high_water(root: &Path) -> Result<Option<u64>, CovyError> {
    let path = generation_high_water_path(root);
    let Some(raw) = read_optional_state_file(&path, MAX_REPO_INDEX_METADATA_BYTES)? else {
        return Ok(None);
    };
    let high_water =
        serde_json::from_slice::<RepoIndexGenerationHighWater>(&raw).map_err(|error| {
            cache_error(format!(
                "failed to decode repository generation high-water '{}': {error}",
                path.display()
            ))
        })?;
    if high_water.schema_version != REPO_INDEX_GENERATION_HIGH_WATER_SCHEMA_VERSION {
        return Err(cache_error(format!(
            "repository generation high-water '{}' has schema {}, expected {}",
            path.display(),
            high_water.schema_version,
            REPO_INDEX_GENERATION_HIGH_WATER_SCHEMA_VERSION
        )));
    }
    Ok(Some(high_water.generation))
}

fn persist_generation_high_water(root: &Path, generation: u64) -> Result<(), CovyError> {
    let encoded = serde_json::to_vec_pretty(&RepoIndexGenerationHighWater {
        schema_version: REPO_INDEX_GENERATION_HIGH_WATER_SCHEMA_VERSION,
        generation,
    })
    .map_err(|error| {
        cache_error(format!(
            "failed to encode repository generation high-water: {error}"
        ))
    })?;
    write_atomic(generation_high_water_path(root), &encoded)
}

fn discover_generation_high_water(root: &Path) -> Result<u64, CovyError> {
    let mut generation = 0;
    for path in [manifest_path(root), previous_manifest_path(root)] {
        if let Some(raw) = read_optional_state_file(&path, MAX_REPO_INDEX_METADATA_BYTES)? {
            if let Ok(manifest) = serde_json::from_slice::<RepoIndexRuntimeManifest>(&raw) {
                generation = generation.max(manifest.generation);
            }
        }
    }
    let directory = repo_index_dir(root);
    let state = match StateDir::open(root, &[".packet28", "index", REPO_INDEX_DIR_NAME], false) {
        Ok(state) => state,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(generation),
        Err(error) => {
            return Err(cache_error(format!(
                "failed to inspect repository index directory '{}': {error}",
                directory.display()
            )));
        }
    };
    for entry in state.names().map_err(|error| {
        cache_error(format!(
            "failed to inspect repository index directory '{}': {error}",
            directory.display()
        ))
    })? {
        let Some(name) = entry.to_str() else {
            continue;
        };
        if let Some(observed) = managed_artifact_generation(name)? {
            generation = generation.max(observed);
        }
    }
    Ok(generation)
}

fn managed_artifact_generation(name: &str) -> Result<Option<u64>, CovyError> {
    let digits = if let Some(digits) = name
        .strip_prefix("generation-")
        .and_then(|name| name.strip_suffix(".json"))
    {
        Some(digits)
    } else if let Some(digits) = name
        .strip_prefix("base-")
        .and_then(|name| name.strip_suffix(".bin"))
    {
        Some(digits)
    } else if let Some(segment) = name
        .strip_prefix("segment-")
        .and_then(|name| name.strip_suffix(".bin"))
    {
        Some(segment.strip_suffix("-compacted").unwrap_or(segment))
    } else {
        None
    };
    let Some(digits) = digits else {
        return Ok(None);
    };
    if digits.len() != 20 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(cache_error(format!(
            "repository generation artifact '{name}' has an invalid generation"
        )));
    }
    let generation = digits.parse::<u64>().map_err(|error| {
        cache_error(format!(
            "repository generation artifact '{name}' has an invalid generation: {error}"
        ))
    })?;
    if generation == 0 {
        return Err(cache_error(format!(
            "repository generation artifact '{name}' uses reserved generation zero"
        )));
    }
    Ok(Some(generation))
}

pub(crate) struct GenerationWriterLock(StateFile);

impl Drop for GenerationWriterLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(self.0.file());
    }
}

pub(crate) fn acquire_writer_lock(root: &Path) -> Result<GenerationWriterLock, CovyError> {
    let parent = repo_index_parent(root);
    let directory = StateDir::open(root, &[".packet28", "index"], true).map_err(|error| {
        cache_error(format!(
            "failed to open repository index parent '{}': {error}",
            parent.display()
        ))
    })?;
    let path = parent.join(REPO_INDEX_WRITER_LOCK_FILE);
    let file = directory
        .open_or_create(REPO_INDEX_WRITER_LOCK_FILE, FileAccess::ReadWrite)
        .map_err(|error| {
            cache_error(format!(
                "failed to open repository index writer lock '{}': {error}",
                path.display()
            ))
        })?
        .file;
    FileExt::lock_exclusive(file.file()).map_err(|error| {
        cache_error(format!(
            "failed to acquire repository index writer lock '{}': {error}",
            path.display()
        ))
    })?;
    if let Err(error) = file.validate_attachment() {
        let _ = FileExt::unlock(file.file());
        return Err(cache_error(format!(
            "repository index writer lock '{}' was replaced while acquiring it: {error}",
            path.display()
        )));
    }
    Ok(GenerationWriterLock(file))
}

fn read_optional_file(path: &Path) -> Result<Option<Vec<u8>>, CovyError> {
    read_optional_state_file(path, MAX_REPO_INDEX_METADATA_BYTES)
}

fn restore_optional_file(path: PathBuf, bytes: Option<&[u8]>) -> Result<(), CovyError> {
    match bytes {
        Some(bytes) => write_atomic(path, bytes),
        None => remove_state_file_if_exists(&path),
    }
}

fn artifact_digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

#[cfg(test)]
fn artifact_stamp(path: &Path) -> Result<ArtifactStamp, CovyError> {
    let metadata = fs::metadata(path).map_err(|error| {
        cache_error(format!(
            "failed to inspect repository index artifact '{}': {error}",
            path.display()
        ))
    })?;
    ensure_regular_artifact(path, &metadata)?;
    Ok(artifact_stamp_from_metadata(&metadata))
}

fn ensure_regular_artifact(path: &Path, metadata: &fs::Metadata) -> Result<(), CovyError> {
    if !metadata.is_file() {
        return Err(cache_error(format!(
            "repository index artifact '{}' is not a regular file",
            path.display()
        )));
    }
    Ok(())
}

fn artifact_stamp_from_metadata(metadata: &fs::Metadata) -> ArtifactStamp {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt as _;

    ArtifactStamp {
        len: metadata.len(),
        modified_unix_nanos: crate::scan::metadata_mtime_unix_nanos(metadata),
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
        #[cfg(unix)]
        changed_unix_nanos: i128::from(metadata.ctime())
            .saturating_mul(1_000_000_000)
            .saturating_add(i128::from(metadata.ctime_nsec())),
    }
}

fn read_authenticated_artifact(
    path: &Path,
    expected_digest: &str,
) -> Result<(Vec<u8>, ArtifactStamp), CovyError> {
    let (directory, name) = repo_state_location(path, false)?;
    let Some(mut file) = directory
        .open_bounded(&name, FileAccess::ReadOnly, MAX_REPO_INDEX_ARTIFACT_BYTES)
        .map_err(|error| {
            cache_error(format!(
                "failed to open repository index artifact '{}': {error}",
                path.display()
            ))
        })?
    else {
        return Err(cache_error(format!(
            "repository index artifact '{}' does not exist",
            path.display()
        )));
    };
    read_authenticated_pinned_artifact(&mut file, path, expected_digest)
}

fn read_authenticated_pinned_artifact(
    file: &mut StateFile,
    path: &Path,
    expected_digest: &str,
) -> Result<(Vec<u8>, ArtifactStamp), CovyError> {
    let before = file
        .file()
        .metadata()
        .map(|metadata| artifact_stamp_from_metadata(&metadata))
        .map_err(|error| {
            cache_error(format!(
                "failed to inspect repository index artifact '{}': {error}",
                path.display()
            ))
        })?;
    let capacity = usize::try_from(before.len).map_err(|_| {
        cache_error(format!(
            "repository index artifact '{}' exceeds the platform allocation limit",
            path.display()
        ))
    })?;
    let mut raw = Vec::new();
    raw.try_reserve_exact(capacity).map_err(|error| {
        cache_error(format!(
            "failed to reserve repository index artifact '{}': {error}",
            path.display()
        ))
    })?;
    file.file_mut().seek(SeekFrom::Start(0)).map_err(|error| {
        cache_error(format!(
            "failed to seek repository index artifact '{}': {error}",
            path.display()
        ))
    })?;
    file.file_mut()
        .take(MAX_REPO_INDEX_ARTIFACT_BYTES.saturating_add(1))
        .read_to_end(&mut raw)
        .map_err(|error| {
            cache_error(format!(
                "failed to read repository index artifact '{}': {error}",
                path.display()
            ))
        })?;
    if u64::try_from(raw.len()).unwrap_or(u64::MAX) > MAX_REPO_INDEX_ARTIFACT_BYTES {
        return Err(cache_error(format!(
            "repository index artifact '{}' exceeds the {}-byte limit",
            path.display(),
            MAX_REPO_INDEX_ARTIFACT_BYTES
        )));
    }
    let after = file
        .file()
        .metadata()
        .map(|metadata| artifact_stamp_from_metadata(&metadata))
        .map_err(|error| {
            cache_error(format!(
                "failed to re-inspect repository index artifact '{}': {error}",
                path.display()
            ))
        })?;
    if before != after {
        return Err(cache_error(format!(
            "repository index artifact '{}' changed while it was being authenticated",
            path.display()
        )));
    }
    file.validate_attachment().map_err(|error| {
        cache_error(format!(
            "repository index artifact '{}' was replaced while it was being authenticated: {error}",
            path.display()
        ))
    })?;
    verify_artifact_digest(path, &raw, expected_digest)?;
    Ok((raw, after))
}

fn validate_loaded_artifact_stamps(
    root: &Path,
    loaded: &RepoIndexGeneration,
) -> Result<ValidatedArtifactStamps, CovyError> {
    let mut metadata_checks = 0usize;
    let mut bytes_hashed = 0usize;
    let (base, base_pin) = pin_loaded_artifact(
        &repo_index_dir(root).join(&loaded.base_file),
        &loaded.base_stamp,
        &loaded.base_digest,
        &mut metadata_checks,
        &mut bytes_hashed,
    )?;
    let mut segments = Vec::with_capacity(loaded.segment_stamps.len());
    let mut pins = Vec::with_capacity(loaded.segment_stamps.len().saturating_add(1));
    pins.push(base_pin);
    for ((file_name, expected_digest), expected_stamp) in loaded
        .segment_files
        .iter()
        .zip(&loaded.segment_digests)
        .zip(&loaded.segment_stamps)
    {
        let (stamp, pin) = pin_loaded_artifact(
            &repo_index_dir(root).join(file_name),
            expected_stamp,
            expected_digest,
            &mut metadata_checks,
            &mut bytes_hashed,
        )?;
        segments.push(stamp);
        pins.push(pin);
    }
    Ok(ValidatedArtifactStamps {
        base,
        segments,
        pins,
        metadata_checks,
        bytes_hashed,
    })
}

fn pin_loaded_artifact(
    path: &Path,
    expected_stamp: &ArtifactStamp,
    expected_digest: &str,
    metadata_checks: &mut usize,
    bytes_hashed: &mut usize,
) -> Result<(ArtifactStamp, PinnedArtifact), CovyError> {
    *metadata_checks = metadata_checks.saturating_add(1);
    let (directory, name) = repo_state_location(path, false)?;
    let Some(mut file) = directory
        .open_bounded(&name, FileAccess::ReadOnly, MAX_REPO_INDEX_ARTIFACT_BYTES)
        .map_err(|error| {
            cache_error(format!(
                "failed to pin repository index artifact '{}': {error}",
                path.display()
            ))
        })?
    else {
        return Err(cache_error(format!(
            "repository index artifact '{}' does not exist",
            path.display()
        )));
    };
    let before_metadata = file.file().metadata().map_err(|error| {
        cache_error(format!(
            "failed to inspect pinned repository index artifact '{}': {error}",
            path.display()
        ))
    })?;
    ensure_regular_artifact(path, &before_metadata)?;
    let before = artifact_stamp_from_metadata(&before_metadata);
    file.validate_attachment().map_err(|error| {
        cache_error(format!(
            "repository index artifact '{}' was replaced while it was being pinned: {error}",
            path.display()
        ))
    })?;
    let stamp = if !cfg!(unix) || before != *expected_stamp {
        let (raw, authenticated_stamp) =
            read_authenticated_pinned_artifact(&mut file, path, expected_digest)?;
        *bytes_hashed = bytes_hashed.saturating_add(raw.len());
        authenticated_stamp
    } else {
        before
    };
    Ok((
        stamp.clone(),
        PinnedArtifact {
            path: path.to_path_buf(),
            file,
            stamp,
            expected_digest: expected_digest.to_string(),
        },
    ))
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
    previous: Option<&RepoIndexGenerationRecord>,
) -> Result<(), CovyError> {
    let mut retained = BTreeSet::from([
        REPO_INDEX_MANIFEST_FILE.to_string(),
        REPO_INDEX_PREVIOUS_MANIFEST_FILE.to_string(),
        generation_record_file_name(current.generation),
        current.base_file.clone(),
    ]);
    retained.extend(current.segment_files.iter().cloned());
    if let Some(previous) = previous.filter(|record| record.generation > 0) {
        retained.insert(generation_record_file_name(previous.generation));
        retained.insert(previous.base_file.clone());
        retained.extend(previous.segment_files.iter().cloned());
    }
    let directory = repo_index_dir(root);
    let state = StateDir::open(root, &[".packet28", "index", REPO_INDEX_DIR_NAME], false).map_err(
        |error| {
            cache_error(format!(
                "failed to inspect repository index directory '{}': {error}",
                directory.display()
            ))
        },
    )?;
    for entry in state.names().map_err(|error| {
        cache_error(format!(
            "failed to inspect repository index directory '{}': {error}",
            directory.display()
        ))
    })? {
        let Some(name) = entry.to_str() else {
            continue;
        };
        if is_managed_generation_artifact(name) && !retained.contains(name) {
            state.remove_file_if_exists(name).map_err(|error| {
                cache_error(format!(
                    "failed to prune repository index artifact '{}': {error}",
                    directory.join(name).display()
                ))
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
fn load_generation_record(
    root: &Path,
    generation: u64,
) -> Result<RepoIndexGenerationRecord, CovyError> {
    let path = repo_index_dir(root).join(generation_record_file_name(generation));
    let raw = read_state_file(&path, MAX_REPO_INDEX_METADATA_BYTES)?;
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

fn repo_state_spec(path: &Path) -> Result<(&Path, &'static [&'static str], String), CovyError> {
    let parent = path.parent().ok_or_else(|| {
        cache_error(format!(
            "repository state path '{}' has no parent",
            path.display()
        ))
    })?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            cache_error(format!(
                "repository state path '{}' has an invalid leaf",
                path.display()
            ))
        })?
        .to_string();
    let (root, components): (&Path, &'static [&'static str]) = if parent
        .file_name()
        .is_some_and(|name| name == REPO_INDEX_DIR_NAME)
    {
        let index = parent
            .parent()
            .filter(|path| path.file_name().is_some_and(|name| name == "index"));
        let packet28 = index
            .and_then(Path::parent)
            .filter(|path| path.file_name().is_some_and(|name| name == ".packet28"));
        let root = packet28.and_then(Path::parent).ok_or_else(|| {
            cache_error(format!(
                "repository state path '{}' is outside the managed index",
                path.display()
            ))
        })?;
        (root, &[".packet28", "index", REPO_INDEX_DIR_NAME])
    } else if parent.file_name().is_some_and(|name| name == "index") {
        let packet28 = parent
            .parent()
            .filter(|path| path.file_name().is_some_and(|name| name == ".packet28"));
        let root = packet28.and_then(Path::parent).ok_or_else(|| {
            cache_error(format!(
                "repository state path '{}' is outside the managed index",
                path.display()
            ))
        })?;
        (root, &[".packet28", "index"])
    } else {
        return Err(cache_error(format!(
            "repository state path '{}' is outside the managed index",
            path.display()
        )));
    };
    Ok((root, components, name))
}

fn repo_state_location(path: &Path, create: bool) -> Result<(StateDir, String), CovyError> {
    let (root, components, name) = repo_state_spec(path)?;
    let directory = StateDir::open(root, components, create).map_err(|error| {
        cache_error(format!(
            "failed to open repository state directory for '{}': {error}",
            path.display()
        ))
    })?;
    Ok((directory, name))
}

fn read_state_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>, CovyError> {
    let (directory, name) = repo_state_location(path, false)?;
    directory
        .read_bounded(&name, max_bytes)
        .map_err(|error| {
            cache_error(format!(
                "failed to read repository state '{}': {error}",
                path.display()
            ))
        })?
        .ok_or_else(|| {
            cache_error(format!(
                "repository state '{}' does not exist",
                path.display()
            ))
        })
}

fn read_optional_state_file(path: &Path, max_bytes: u64) -> Result<Option<Vec<u8>>, CovyError> {
    let (root, components, name) = repo_state_spec(path)?;
    let directory = match StateDir::open(root, components, false) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(cache_error(format!(
                "failed to open repository state directory for '{}': {error}",
                path.display()
            )));
        }
    };
    directory.read_bounded(&name, max_bytes).map_err(|error| {
        cache_error(format!(
            "failed to read repository state '{}': {error}",
            path.display()
        ))
    })
}

fn remove_state_file_if_exists(path: &Path) -> Result<(), CovyError> {
    let (root, components, name) = repo_state_spec(path)?;
    let directory = match StateDir::open(root, components, false) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(cache_error(format!(
                "failed to open repository state directory for '{}': {error}",
                path.display()
            )));
        }
    };
    directory.remove_file_if_exists(&name).map_err(|error| {
        cache_error(format!(
            "failed to remove repository state '{}': {error}",
            path.display()
        ))
    })
}

fn write_immutable(path: PathBuf, bytes: &[u8]) -> Result<(), CovyError> {
    let (directory, name) = repo_state_location(&path, true)?;
    directory.write_immutable(&name, bytes).map_err(|error| {
        cache_error(format!(
            "failed to create immutable repository artifact '{}': {error}",
            path.display()
        ))
    })
}

fn write_atomic(path: PathBuf, bytes: &[u8]) -> Result<(), CovyError> {
    let (directory, name) = repo_state_location(&path, true)?;
    directory.write_atomic(&name, bytes).map_err(|error| {
        cache_error(format!(
            "failed to publish repository artifact '{}': {error}",
            path.display()
        ))
    })
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

fn repo_index_parent(root: &Path) -> PathBuf {
    root.join(".packet28").join("index")
}

fn repo_index_dir(root: &Path) -> PathBuf {
    repo_index_parent(root).join(REPO_INDEX_DIR_NAME)
}

fn generation_high_water_path(root: &Path) -> PathBuf {
    repo_index_parent(root).join(REPO_INDEX_GENERATION_HIGH_WATER_FILE)
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
    use std::fs::OpenOptions;
    use std::io::Write;
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

    fn generation_artifact_snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
        let directory = repo_index_dir(root);
        if !directory.exists() {
            return BTreeMap::new();
        }
        fs::read_dir(directory)
            .expect("generation directory")
            .map(|entry| {
                let entry = entry.expect("generation entry");
                (
                    entry.file_name().to_string_lossy().to_string(),
                    fs::read(entry.path()).expect("generation artifact"),
                )
            })
            .collect()
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
    fn runtime_map_reflects_incremental_update_without_repository_rescan() {
        let dir = fixture();
        let root = dir.path();
        let current = rebuild_repo_index_runtime(root, true).expect("current");
        fs::write(
            root.join("src/a.rs"),
            "pub fn alpha() {}\npub fn incremental_symbol() {}\n",
        )
        .expect("incremental edit");
        let updated = update_repo_index_runtime(root, &current, &[String::from("src/a.rs")], true)
            .expect("incremental update")
            .0;
        fs::rename(root.join("src"), root.join("source-unavailable"))
            .expect("make repository scan unavailable");

        let envelope = crate::build_repo_map_from_runtime(
            crate::RepoMapRequest {
                repo_root: root.to_string_lossy().to_string(),
                focus_symbols: vec![String::from("incremental_symbol")],
                max_files: 8,
                max_symbols: 16,
                include_tests: true,
                ..crate::RepoMapRequest::default()
            },
            &updated,
        )
        .expect("runtime map");
        let rich = crate::expand_repo_map_payload(&envelope);

        assert!(rich
            .symbols_ranked
            .iter()
            .any(|symbol| symbol.name == "incremental_symbol" && symbol.file == "src/a.rs"));
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
    fn corrupt_manifest_repair_never_reuses_an_identity_or_accepts_its_stale_handle() {
        let dir = fixture();
        let root = dir.path();
        let stale = rebuild_repo_index_runtime(root, true).expect("first");
        let first_record =
            load_generation_record(root, stale.manifest.generation).expect("first record");
        let first_record_path =
            repo_index_dir(root).join(generation_record_file_name(stale.manifest.generation));
        let first_base_path = repo_index_dir(root).join(&first_record.base_file);
        let first_record_bytes = fs::read(&first_record_path).expect("first record bytes");
        let first_base_bytes = fs::read(&first_base_path).expect("first base bytes");
        fs::write(root.join("src/a.rs"), "pub fn second() {}\n").expect("second contents");
        let second = rebuild_repo_index_runtime(root, true).expect("second");
        fs::remove_file(generation_high_water_path(root)).expect("simulate legacy allocator");
        fs::write(manifest_path(root), b"{").expect("corrupt current manifest");

        let recovered = load_repo_index_runtime(root).expect("recover first");
        fs::write(root.join("src/c.rs"), "pub struct Repaired;\n").expect("repair contents");
        let repaired = rebuild_repo_index_runtime(root, true).expect("repair");

        assert_eq!(recovered.manifest.generation, stale.manifest.generation);
        assert!(repaired.manifest.generation > second.manifest.generation);
        assert_eq!(
            fs::read(first_record_path).expect("retained first record"),
            first_record_bytes
        );
        assert_eq!(
            fs::read(first_base_path).expect("retained first base"),
            first_base_bytes
        );

        fs::write(root.join("src/d.rs"), "pub struct StaleWriter;\n").expect("stale path");
        let before = generation_artifact_snapshot(root);
        let counter_before =
            fs::read(generation_high_water_path(root)).expect("counter before conflict");
        let error = update_repo_index_runtime(root, &stale, &[String::from("src/d.rs")], true)
            .expect_err("stale writer must conflict");

        assert!(error.to_string().contains("generation conflict"));
        assert_eq!(generation_artifact_snapshot(root), before);
        assert_eq!(
            fs::read(generation_high_water_path(root)).expect("counter after conflict"),
            counter_before
        );
        assert_eq!(
            load_repo_index_runtime(root)
                .expect("load repaired")
                .manifest
                .generation,
            repaired.manifest.generation
        );
    }

    #[test]
    fn missing_or_schema_zero_current_retains_the_authenticated_previous_generation() {
        for current_bytes in [None, Some(b"{}".as_slice())] {
            let dir = fixture();
            let root = dir.path();
            let first = rebuild_repo_index_runtime(root, true).expect("first");
            fs::write(root.join("src/c.rs"), "pub struct Second;\n").expect("second file");
            let second = rebuild_repo_index_runtime(root, true).expect("second");
            match current_bytes {
                Some(bytes) => fs::write(manifest_path(root), bytes).expect("schema-zero current"),
                None => fs::remove_file(manifest_path(root)).expect("missing current"),
            }

            let recovered = load_repo_index_runtime(root).expect("recover previous");
            assert_eq!(recovered.manifest.generation, first.manifest.generation);
            let repaired = rebuild_repo_index_runtime(root, true).expect("repair");
            assert!(repaired.manifest.generation > second.manifest.generation);
            let repaired_record = current_record(root);
            fs::write(
                repo_index_dir(root).join(repaired_record.base_file),
                b"corrupt repair",
            )
            .expect("corrupt repair artifact");

            assert_eq!(
                load_repo_index_runtime(root)
                    .expect("recover retained previous")
                    .manifest
                    .generation,
                first.manifest.generation
            );
        }
    }

    #[test]
    fn clear_migrates_legacy_fencing_and_rejects_a_pre_clear_handle() {
        let dir = fixture();
        let root = dir.path();
        let stale = rebuild_repo_index_runtime(root, true).expect("first");
        fs::remove_file(generation_high_water_path(root)).expect("simulate legacy allocator");

        clear_repo_index_runtime(root).expect("clear");

        assert_eq!(
            load_generation_high_water(root).expect("migrated high-water"),
            Some(stale.manifest.generation)
        );
        assert!(stale.file("src/a.rs").is_some());
        let rebuilt = rebuild_repo_index_runtime(root, true).expect("rebuild");
        assert!(rebuilt.manifest.generation > stale.manifest.generation);

        fs::write(root.join("src/a.rs"), "pub fn stale_after_clear() {}\n").expect("change");
        let error = update_repo_index_runtime(root, &stale, &[String::from("src/a.rs")], true)
            .expect_err("pre-clear writer must conflict");
        assert!(error.to_string().contains("generation conflict"));
        assert_eq!(
            load_repo_index_runtime(root)
                .expect("load rebuilt")
                .manifest
                .generation,
            rebuilt.manifest.generation
        );
    }

    #[test]
    fn clear_fails_closed_when_generation_fencing_is_corrupt() {
        let dir = fixture();
        let root = dir.path();
        let current = rebuild_repo_index_runtime(root, true).expect("current");
        fs::write(generation_high_water_path(root), b"{").expect("corrupt high-water");
        let before = generation_artifact_snapshot(root);

        let error = clear_repo_index_runtime(root).expect_err("strict high-water");

        assert!(error.to_string().contains("generation high-water"));
        assert_eq!(generation_artifact_snapshot(root), before);
        assert_eq!(
            load_repo_index_runtime(root)
                .expect("load current")
                .manifest
                .generation,
            current.manifest.generation
        );
    }

    #[test]
    fn writer_compare_and_swap_uses_the_private_authenticated_identity() {
        let dir = fixture();
        let root = dir.path();
        let current = rebuild_repo_index_runtime(root, true).expect("current");
        let mut stale = current.clone();
        Arc::make_mut(stale.generation.as_mut().expect("loaded generation"))
            .publication_identity
            .generation_record_digest = "stale-publication".to_string();
        stale.manifest = current.manifest.clone();
        fs::write(root.join("src/a.rs"), "pub fn attempted() {}\n").expect("change");
        let before = generation_artifact_snapshot(root);
        let counter_before =
            fs::read(generation_high_water_path(root)).expect("counter before conflict");

        let error = update_repo_index_runtime(root, &stale, &[String::from("src/a.rs")], true)
            .expect_err("identity mismatch");

        assert!(error.to_string().contains("generation conflict"));
        assert_eq!(generation_artifact_snapshot(root), before);
        assert_eq!(
            fs::read(generation_high_water_path(root)).expect("counter after conflict"),
            counter_before
        );
    }

    #[test]
    fn incremental_identity_check_decodes_only_bounded_publication_metadata() {
        let dir = fixture();
        let root = dir.path();
        let mut current = rebuild_repo_index_runtime(root, true).expect("current");
        for revision in 0..4 {
            fs::write(
                root.join("src/a.rs"),
                format!("pub fn alpha_revision_{revision}() {{}}\n"),
            )
            .expect("seed overlay");
            current = update_repo_index_runtime(root, &current, &[String::from("src/a.rs")], true)
                .expect("seed update")
                .0;
        }
        fs::write(root.join("src/a.rs"), "pub fn measured_update() {}\n").expect("measured edit");

        let (updated, summary) =
            update_repo_index_runtime(root, &current, &[String::from("src/a.rs")], true)
                .expect("measured update");

        assert!(updated.shares_base_with(&current));
        assert_eq!(summary.work.repository_artifact_bytes_decoded, 0);
        assert_eq!(summary.work.repository_artifacts_decoded, 0);
        #[cfg(unix)]
        assert_eq!(summary.work.repository_artifact_bytes_hashed, 0);
        #[cfg(not(unix))]
        assert!(
            summary.work.repository_artifact_bytes_hashed > 0,
            "platforms without a stable file identity must rehash retained artifacts"
        );
        assert_eq!(summary.work.repository_artifact_metadata_checks, 18);
        assert_eq!(summary.work.changed_paths_considered, 1);
        assert!(
            (1..=4_096).contains(&summary.work.publication_metadata_bytes_decoded),
            "publication identity should decode only the small manifest and generation record, decoded {} bytes",
            summary.work.publication_metadata_bytes_decoded
        );
    }

    #[test]
    fn incremental_identity_check_rejects_manifest_metadata_tampering() {
        let dir = fixture();
        let root = dir.path();
        let current = rebuild_repo_index_runtime(root, true).expect("current");
        let mut tampered = load_published_manifest(root).expect("manifest");
        tampered.total_files += 1;
        fs::write(
            manifest_path(root),
            serde_json::to_vec_pretty(&tampered).expect("encode tampered manifest"),
        )
        .expect("tamper manifest");
        fs::write(root.join("src/a.rs"), "pub fn attempted() {}\n").expect("change");
        let counter_before =
            fs::read(generation_high_water_path(root)).expect("counter before rejection");

        let error = update_repo_index_runtime(root, &current, &[String::from("src/a.rs")], true)
            .expect_err("tampered manifest must fail authentication");

        assert!(error.to_string().contains("generation record"));
        assert!(error.to_string().contains("does not match"));
        assert_eq!(
            fs::read(generation_high_water_path(root)).expect("counter after rejection"),
            counter_before
        );
    }

    #[test]
    fn mutable_manifest_policy_cannot_bypass_the_private_loaded_base_policy() {
        let dir = fixture();
        let root = dir.path();
        let current = rebuild_repo_index_runtime(root, true).expect("current");
        let mut mutated = current.clone();
        mutated.manifest.include_tests = false;
        assert!(
            mutated
                .materialize_snapshot()
                .expect("private snapshot")
                .include_tests
        );

        let rebuilt = update_repo_index_runtime(root, &mutated, &[], false)
            .expect("policy rebuild")
            .0;

        assert!(rebuilt.manifest.generation > current.manifest.generation);
        assert!(!rebuilt.manifest.include_tests);
        assert!(!current.shares_base_with(&rebuilt));
        let reloaded = load_repo_index_runtime(root).expect("reload rebuilt policy");
        assert!(reloaded.is_loaded());
        assert!(!reloaded.manifest.include_tests);

        let mut stale = current;
        stale.manifest.include_tests = false;
        let before = generation_artifact_snapshot(root);
        let counter_before =
            fs::read(generation_high_water_path(root)).expect("counter before stale policy change");
        let error = update_repo_index_runtime(root, &stale, &[], false)
            .expect_err("stale policy change must conflict");
        assert!(error.to_string().contains("generation conflict"));
        assert_eq!(generation_artifact_snapshot(root), before);
        assert_eq!(
            fs::read(generation_high_water_path(root)).expect("counter after stale policy change"),
            counter_before
        );
    }

    #[test]
    fn generation_high_water_reserves_max_once_then_exhausts_without_artifact_writes() {
        let dir = fixture();
        let root = dir.path();
        persist_generation_high_water(root, u64::MAX - 1).expect("seed high-water");
        let _writer = acquire_writer_lock(root).expect("writer");

        assert_eq!(reserve_generation(root).expect("reserve max"), u64::MAX);
        let counter_at_max = fs::read(generation_high_water_path(root)).expect("counter at max");
        let error = reserve_generation(root).expect_err("generation exhaustion");

        assert!(error.to_string().contains("exhausted"));
        assert_eq!(
            fs::read(generation_high_water_path(root)).expect("counter after exhaustion"),
            counter_at_max
        );
        assert!(!repo_index_dir(root).exists());
    }

    #[test]
    fn rebuild_at_generation_max_fails_before_creating_generation_artifacts() {
        let dir = fixture();
        let root = dir.path();
        persist_generation_high_water(root, u64::MAX).expect("seed max");
        let counter_before =
            fs::read(generation_high_water_path(root)).expect("counter before rebuild");

        let error = rebuild_repo_index_runtime(root, true).expect_err("generation exhaustion");

        assert!(error.to_string().contains("exhausted"));
        assert!(!repo_index_dir(root).exists());
        assert_eq!(
            fs::read(generation_high_water_path(root)).expect("counter after rebuild"),
            counter_before
        );
    }

    #[test]
    fn update_at_generation_max_preserves_the_published_generation() {
        let dir = fixture();
        let root = dir.path();
        let current = rebuild_repo_index_runtime(root, true).expect("current");
        persist_generation_high_water(root, u64::MAX).expect("seed max");
        fs::write(root.join("src/a.rs"), "pub fn overflow_attempt() {}\n").expect("change");
        let before = generation_artifact_snapshot(root);
        let counter_before =
            fs::read(generation_high_water_path(root)).expect("counter before update");

        let error = update_repo_index_runtime(root, &current, &[String::from("src/a.rs")], true)
            .expect_err("generation exhaustion");

        assert!(error.to_string().contains("exhausted"));
        assert_eq!(generation_artifact_snapshot(root), before);
        assert_eq!(
            fs::read(generation_high_water_path(root)).expect("counter after update"),
            counter_before
        );
        assert_eq!(
            load_repo_index_runtime(root)
                .expect("load current")
                .manifest
                .generation,
            current.manifest.generation
        );
    }

    #[test]
    fn update_does_not_discard_recovery_when_the_published_artifact_is_corrupt() {
        let dir = fixture();
        let root = dir.path();
        let first = rebuild_repo_index_runtime(root, true).expect("first");
        fs::write(root.join("src/c.rs"), "pub struct Second;\n").expect("second file");
        let second = rebuild_repo_index_runtime(root, true).expect("second");
        let second_record = current_record(root);
        fs::write(
            repo_index_dir(root).join(second_record.base_file),
            b"corrupt second base",
        )
        .expect("corrupt current base");
        fs::write(root.join("src/d.rs"), "pub struct Attempt;\n").expect("update path");
        let before = generation_artifact_snapshot(root);
        let counter_before =
            fs::read(generation_high_water_path(root)).expect("counter before update");

        let error = update_repo_index_runtime(root, &second, &[String::from("src/d.rs")], true)
            .expect_err("corrupt published generation");

        assert!(error.to_string().contains("digest validation"));
        assert_eq!(generation_artifact_snapshot(root), before);
        assert_eq!(
            fs::read(generation_high_water_path(root)).expect("counter after update"),
            counter_before
        );
        assert_eq!(
            load_repo_index_runtime(root)
                .expect("recover first")
                .manifest
                .generation,
            first.manifest.generation
        );
    }

    #[test]
    fn update_rejects_a_corrupt_published_record_without_displacing_recovery() {
        let dir = fixture();
        let root = dir.path();
        let first = rebuild_repo_index_runtime(root, true).expect("first");
        fs::write(root.join("src/c.rs"), "pub struct Second;\n").expect("second file");
        let second = rebuild_repo_index_runtime(root, true).expect("second");
        let record_path =
            repo_index_dir(root).join(generation_record_file_name(second.manifest.generation));
        fs::write(&record_path, b"{").expect("corrupt current record");
        fs::write(root.join("src/d.rs"), "pub struct Attempt;\n").expect("update path");
        let before = generation_artifact_snapshot(root);
        let counter_before =
            fs::read(generation_high_water_path(root)).expect("counter before update");

        let error = update_repo_index_runtime(root, &second, &[String::from("src/d.rs")], true)
            .expect_err("corrupt published record");

        assert!(error.to_string().contains("generation record"));
        assert_eq!(generation_artifact_snapshot(root), before);
        assert_eq!(
            fs::read(generation_high_water_path(root)).expect("counter after update"),
            counter_before
        );
        assert_eq!(
            load_repo_index_runtime(root)
                .expect("recover first")
                .manifest
                .generation,
            first.manifest.generation
        );
    }

    #[cfg(unix)]
    #[test]
    fn update_rehashes_same_size_artifact_corruption_with_restored_mtime() {
        let dir = fixture();
        let root = dir.path();
        let first = rebuild_repo_index_runtime(root, true).expect("first");
        fs::write(root.join("src/c.rs"), "pub struct Second;\n").expect("second file");
        let second = rebuild_repo_index_runtime(root, true).expect("second");
        let loaded = second.generation.as_ref().expect("loaded second");
        let base_path = repo_index_dir(root).join(&loaded.base_file);
        let expected_stamp = loaded.base_stamp.clone();
        let timestamp_reference = root.join(".artifact-mtime-reference");
        fs::write(&timestamp_reference, b"reference").expect("timestamp reference");
        let status = std::process::Command::new("touch")
            .args(["-r"])
            .arg(&base_path)
            .arg(&timestamp_reference)
            .status()
            .expect("copy original mtime");
        assert!(status.success());
        let mut corrupted = fs::read(&base_path).expect("base bytes");
        corrupted[0] ^= 0xff;
        fs::write(&base_path, corrupted).expect("same-size corruption");
        let status = std::process::Command::new("touch")
            .args(["-r"])
            .arg(&timestamp_reference)
            .arg(&base_path)
            .status()
            .expect("restore original mtime");
        assert!(status.success());
        let corrupted_stamp = artifact_stamp(&base_path).expect("corrupted stamp");
        assert_eq!(corrupted_stamp.len, expected_stamp.len);
        assert_eq!(
            corrupted_stamp.modified_unix_nanos,
            expected_stamp.modified_unix_nanos
        );
        assert_ne!(
            corrupted_stamp, expected_stamp,
            "the Unix change token must detect same-size, same-mtime replacement"
        );
        fs::write(root.join("src/d.rs"), "pub struct Attempt;\n").expect("update path");
        let before = generation_artifact_snapshot(root);

        let error = update_repo_index_runtime(root, &second, &[String::from("src/d.rs")], true)
            .expect_err("same-size corrupt published artifact");

        assert!(error.to_string().contains("digest validation"));
        assert_eq!(generation_artifact_snapshot(root), before);
        assert_eq!(
            load_repo_index_runtime(root)
                .expect("recover first")
                .manifest
                .generation,
            first.manifest.generation
        );
    }

    #[test]
    fn incremental_publication_fence_restores_both_manifests_before_pruning() {
        let dir = fixture();
        let root = dir.path();
        let mut current = rebuild_repo_index_runtime(root, true).expect("base");
        for revision in 0..7 {
            fs::write(
                root.join("src/a.rs"),
                format!("pub fn alpha_revision_{revision}() {{}}\n"),
            )
            .expect("seed segment");
            current = update_repo_index_runtime(root, &current, &[String::from("src/a.rs")], true)
                .expect("seed update")
                .0;
        }
        assert_eq!(current.manifest.segment_count, 7);
        let recovery_manifest = load_previous_manifest(root).expect("recovery manifest");
        let recovery_record =
            load_authenticated_generation_record(root, &recovery_manifest).expect("recovery");
        let current_manifest_bytes = fs::read(manifest_path(root)).expect("current bytes");
        let previous_manifest_bytes =
            fs::read(previous_manifest_path(root)).expect("previous bytes");
        let current_record_path =
            repo_index_dir(root).join(generation_record_file_name(current.manifest.generation));
        fs::write(root.join("src/a.rs"), "pub fn compacted_update() {}\n").expect("changed path");

        let error = update_repo_index_runtime_with_hook(
            root,
            &current,
            &[String::from("src/a.rs")],
            true,
            |stage| {
                if stage == IncrementalPublicationStage::AfterPrevious {
                    let mut swapped =
                        load_generation_record(root, current.manifest.generation).expect("record");
                    swapped.segment_files.clear();
                    swapped.segment_digests.clear();
                    fs::write(
                        &current_record_path,
                        serde_json::to_vec_pretty(&swapped).expect("swapped record"),
                    )
                    .expect("swap retained record");
                }
                Ok(())
            },
        )
        .expect_err("post-rotation record swap must fail");

        assert!(error.to_string().contains("changed during publication"));
        assert_eq!(
            fs::read(manifest_path(root)).expect("restored current"),
            current_manifest_bytes
        );
        assert_eq!(
            fs::read(previous_manifest_path(root)).expect("restored previous"),
            previous_manifest_bytes
        );
        let recovered = load_repo_index_runtime(root).expect("recover retained generation");
        assert_eq!(recovered.manifest.generation, recovery_manifest.generation);
        for file_name in
            std::iter::once(&recovery_record.base_file).chain(recovery_record.segment_files.iter())
        {
            assert!(
                repo_index_dir(root).join(file_name).exists(),
                "recovery artifact {file_name} must not be pruned"
            );
        }
    }

    #[test]
    fn incremental_publication_authenticates_the_new_record_before_returning() {
        let dir = fixture();
        let root = dir.path();
        let current = rebuild_repo_index_runtime(root, true).expect("current");
        let current_manifest_bytes = fs::read(manifest_path(root)).expect("current bytes");
        let previous_manifest_bytes =
            read_optional_file(&previous_manifest_path(root)).expect("previous bytes");
        let next_record_path = repo_index_dir(root).join(generation_record_file_name(
            current.manifest.generation.checked_add(1).expect("next"),
        ));
        fs::write(root.join("src/a.rs"), "pub fn attempted_update() {}\n").expect("changed path");

        let error = update_repo_index_runtime_with_hook(
            root,
            &current,
            &[String::from("src/a.rs")],
            true,
            |stage| {
                if stage == IncrementalPublicationStage::BeforeCurrent {
                    OpenOptions::new()
                        .append(true)
                        .open(&next_record_path)
                        .expect("open next record")
                        .write_all(b"\n")
                        .expect("pad next record");
                }
                Ok(())
            },
        )
        .expect_err("mutated new record must fail");

        assert!(error.to_string().contains("changed during publication"));
        assert_eq!(
            fs::read(manifest_path(root)).expect("restored current"),
            current_manifest_bytes
        );
        assert_eq!(
            read_optional_file(&previous_manifest_path(root)).expect("restored previous"),
            previous_manifest_bytes
        );
    }

    #[test]
    fn incremental_publication_fence_pins_the_new_segment_through_publication() {
        let dir = fixture();
        let root = dir.path();
        let current = rebuild_repo_index_runtime(root, true).expect("current");
        let current_manifest_bytes = fs::read(manifest_path(root)).expect("current bytes");
        let previous_manifest_bytes =
            read_optional_file(&previous_manifest_path(root)).expect("previous bytes");
        let next_segment_path = repo_index_dir(root).join(segment_file_name(
            current.manifest.generation.checked_add(1).expect("next"),
        ));
        fs::write(root.join("src/a.rs"), "pub fn attempted_update() {}\n").expect("changed path");

        let error = update_repo_index_runtime_with_hook(
            root,
            &current,
            &[String::from("src/a.rs")],
            true,
            |stage| {
                if stage == IncrementalPublicationStage::AfterCurrent {
                    let mut bytes = fs::read(&next_segment_path).expect("next segment");
                    bytes[0] ^= 0xff;
                    fs::write(&next_segment_path, bytes).expect("mutate next segment");
                }
                Ok(())
            },
        )
        .expect_err("mutated new segment must fail");

        assert!(error.to_string().contains("changed during publication"));
        assert_eq!(
            fs::read(manifest_path(root)).expect("restored current"),
            current_manifest_bytes
        );
        assert_eq!(
            read_optional_file(&previous_manifest_path(root)).expect("restored previous"),
            previous_manifest_bytes
        );
        assert_eq!(
            load_repo_index_runtime(root)
                .expect("load restored current")
                .manifest
                .generation,
            current.manifest.generation
        );
    }

    #[test]
    fn generation_record_padding_is_bounded_and_noncanonical() {
        for padding in [1usize, MAX_GENERATION_RECORD_BYTES as usize + 1] {
            let dir = fixture();
            let root = dir.path();
            let current = rebuild_repo_index_runtime(root, true).expect("current");
            let record_path =
                repo_index_dir(root).join(generation_record_file_name(current.manifest.generation));
            OpenOptions::new()
                .append(true)
                .open(&record_path)
                .expect("open record")
                .write_all(&vec![b' '; padding])
                .expect("pad record");
            let loaded = load_repo_index_runtime(root).expect("bounded corrupt load");
            assert!(!loaded.is_loaded());
            let load_error = loaded.manifest.last_error.as_deref().unwrap_or_default();
            if padding == 1 {
                assert!(load_error.contains("canonical persisted encoding"));
            } else {
                assert!(load_error.contains("maximum"));
            }
            fs::write(root.join("src/a.rs"), "pub fn attempted() {}\n").expect("changed path");
            let counter_before =
                fs::read(generation_high_water_path(root)).expect("counter before");

            let error =
                update_repo_index_runtime(root, &current, &[String::from("src/a.rs")], true)
                    .expect_err("padded record must fail");

            if padding == 1 {
                assert!(error.to_string().contains("canonical persisted encoding"));
            } else {
                assert!(error.to_string().contains("maximum"));
            }
            assert_eq!(
                fs::read(generation_high_water_path(root)).expect("counter after"),
                counter_before
            );
        }
    }

    #[test]
    fn corrupt_generation_high_water_fails_closed_without_touching_the_index() {
        let dir = fixture();
        let root = dir.path();
        let current = rebuild_repo_index_runtime(root, true).expect("current");
        fs::write(generation_high_water_path(root), b"{").expect("corrupt high-water");
        let before = generation_artifact_snapshot(root);

        let error = rebuild_repo_index_runtime(root, true).expect_err("strict high-water");

        assert!(error.to_string().contains("generation high-water"));
        assert_eq!(generation_artifact_snapshot(root), before);
        assert_eq!(
            load_repo_index_runtime(root)
                .expect("load current")
                .manifest
                .generation,
            current.manifest.generation
        );
    }

    #[test]
    fn immutable_artifact_write_refuses_to_replace_existing_bytes() {
        let dir = fixture();
        let path = repo_index_dir(dir.path()).join(base_file_name(1));
        fs::create_dir_all(path.parent().expect("artifact parent")).expect("create parent");
        fs::write(&path, b"retained generation").expect("sentinel");

        let error = write_immutable(path.clone(), b"replacement").expect_err("collision");

        assert!(error.to_string().contains("immutable repository artifact"));
        assert_eq!(
            fs::read(path).expect("retained bytes"),
            b"retained generation"
        );
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_skips_a_precreated_temporary_symlink_without_following_it() {
        let dir = fixture();
        let target = manifest_path(dir.path());
        let outside = dir.path().join("outside");
        fs::write(&outside, b"sentinel").expect("outside sentinel");
        let (state, name) = repo_state_location(&target, true).expect("state");
        let mut collision = None;
        state
            .write_atomic_with_observers(
                &name,
                b"published",
                |candidate| {
                    if collision.is_none() {
                        std::os::unix::fs::symlink(&outside, candidate)?;
                        collision = Some(candidate.to_path_buf());
                    }
                    Ok(())
                },
                |_| Ok(()),
                |_, _| Ok(()),
            )
            .expect("atomic write");

        assert_eq!(fs::read(outside).expect("outside bytes"), b"sentinel");
        assert!(collision.expect("collision").is_symlink());
        assert_eq!(fs::read(target).expect("published bytes"), b"published");
    }

    #[cfg(unix)]
    #[test]
    fn rebuild_rejects_a_symlinked_state_parent_without_touching_the_victim() {
        let dir = fixture();
        let victim = tempfile::tempdir().expect("victim");
        let sentinel = victim.path().join("sentinel");
        fs::write(&sentinel, b"outside-must-survive").expect("sentinel");
        std::os::unix::fs::symlink(victim.path(), dir.path().join(".packet28"))
            .expect("symlink parent");

        let error = rebuild_repo_index_runtime(dir.path(), true).expect_err("unsafe parent");

        assert!(error.to_string().contains("repository index parent"));
        assert_eq!(
            fs::read(&sentinel).expect("sentinel bytes"),
            b"outside-must-survive"
        );
        assert!(!victim.path().join("index").exists());
    }

    #[cfg(unix)]
    #[test]
    fn retained_map_state_rejects_an_ancestor_swap_without_touching_the_victim() {
        let dir = fixture();
        let victim = tempfile::tempdir().expect("victim");
        let target = manifest_path(dir.path());
        let (state, name) = repo_state_location(&target, true).expect("state capability");
        let held = dir.path().join("held-packet28");
        let sentinel = victim.path().join("sentinel");
        fs::write(&sentinel, b"outside-must-survive").expect("sentinel");
        fs::rename(dir.path().join(".packet28"), &held).expect("hold state");
        std::os::unix::fs::symlink(victim.path(), dir.path().join(".packet28"))
            .expect("swap ancestor");

        let error = state
            .write_atomic(&name, b"replacement")
            .expect_err("substituted ancestor");

        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::NotADirectory | std::io::ErrorKind::PermissionDenied
        ));
        assert_eq!(
            fs::read(&sentinel).expect("sentinel bytes"),
            b"outside-must-survive"
        );
        assert!(!victim.path().join(REPO_INDEX_MANIFEST_FILE).exists());
        assert!(!held.join("index/mapy-v1/manifest.json").exists());
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
    fn repair_retains_the_authenticated_recovery_generation_instead_of_corrupt_current() {
        let dir = fixture();
        let root = dir.path();
        let first = rebuild_repo_index_runtime(root, true).expect("first");
        fs::write(root.join("src/c.rs"), "pub struct CorruptCurrent;\n").expect("second file");
        let second = rebuild_repo_index_runtime(root, true).expect("second");
        let second_record = current_record(root);
        fs::write(
            repo_index_dir(root).join(second_record.base_file),
            b"corrupt second base",
        )
        .expect("corrupt second");
        let recovered = load_repo_index_runtime(root).expect("recover first");
        assert_eq!(recovered.manifest.generation, first.manifest.generation);

        fs::write(root.join("src/d.rs"), "pub struct Repair;\n").expect("repair file");
        let repaired = rebuild_repo_index_runtime(root, true).expect("repair");
        assert!(repaired.manifest.generation > second.manifest.generation);
        let repaired_record = current_record(root);
        fs::write(
            repo_index_dir(root).join(repaired_record.base_file),
            b"corrupt repaired base",
        )
        .expect("corrupt repaired");

        let recovered_again = load_repo_index_runtime(root).expect("recover retained first");

        assert_eq!(
            recovered_again.manifest.generation,
            first.manifest.generation
        );
        assert!(recovered_again.file("src/c.rs").is_none());
        assert!(recovered_again.file("src/d.rs").is_none());
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
