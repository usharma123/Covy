//! Incremental, crash-safe repository-index generations.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use suite_packet_core::CovyError;

use crate::runtime::{index_repo_path, normalize_path, MAP_CACHE_VERSION};
use crate::{build_repo_index, RepoIndexFileEntry, RepoIndexSnapshot, RepoIndexUpdateSummary};

const REPO_INDEX_RUNTIME_SCHEMA_VERSION: u32 = 1;
const REPO_INDEX_DIR_NAME: &str = "mapy-v1";
const REPO_INDEX_MANIFEST_FILE: &str = "manifest.json";
const REPO_INDEX_PREVIOUS_MANIFEST_FILE: &str = "manifest.previous.json";
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

#[derive(Debug)]
struct RepoIndexGeneration {
    base: Arc<RepoIndexSnapshot>,
    base_file: String,
    segments: Vec<Arc<RepoIndexSegment>>,
    segment_files: Vec<String>,
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
    segment_files: Vec<String>,
    manifest: RepoIndexRuntimeManifest,
}

/// Builds and atomically publishes a new immutable base generation.
///
/// The old manifest stays authoritative until every new artifact has been
/// written, flushed, reloaded, and validated.
///
/// # Errors
///
/// Returns [`CovyError::Other`] for repository scan failures and
/// [`CovyError::Cache`] for encoding, persistence, or validation failures.
pub fn rebuild_repo_index_runtime(
    root: &Path,
    include_tests: bool,
) -> Result<RepoIndexRuntime, CovyError> {
    let snapshot = build_repo_index(root, include_tests)?;
    let previous = load_published_manifest(root).ok();
    let generation = next_generation(previous.as_ref(), None);
    let base_file = base_file_name(generation);
    let base_bytes = wincode::serialize(&snapshot)
        .map_err(|error| cache_error(format!("failed to encode repository base: {error}")))?;
    write_atomic(repo_index_dir(root).join(&base_file), &base_bytes)?;

    let manifest = RepoIndexRuntimeManifest {
        schema_version: REPO_INDEX_RUNTIME_SCHEMA_VERSION,
        generation,
        include_tests,
        total_files: snapshot.files.len(),
        overlay_files: 0,
        segment_count: 0,
        status: "ready".to_string(),
        recovered_from_generation: None,
        last_error: None,
    };
    let record = RepoIndexGenerationRecord {
        schema_version: REPO_INDEX_RUNTIME_SCHEMA_VERSION,
        generation,
        base_file,
        segment_files: Vec::new(),
        manifest: manifest.clone(),
    };
    persist_generation_record(root, &record)?;
    let runtime = load_generation(root, &manifest)?;
    publish_manifest(root, previous.as_ref(), &manifest)?;
    Ok(runtime)
}

/// Publishes changed paths as one immutable delta segment.
///
/// At eight referenced segments, live overlay entries are compacted into one
/// segment. The immutable base remains shared across old and new readers.
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
    if current.manifest.include_tests != include_tests {
        let rebuilt = rebuild_repo_index_runtime(root, include_tests)?;
        let changed_paths = normalize_changed_paths(changed_paths);
        return Ok((
            rebuilt,
            RepoIndexUpdateSummary {
                indexed_files: 0,
                removed_files: 0,
                changed_paths,
            },
        ));
    }
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

    let generation = next_generation(
        load_published_manifest(root).ok().as_ref(),
        Some(current.manifest.generation),
    );
    let normalized = normalize_changed_paths(changed_paths);
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
    let segment_file = segment_file_name(generation);
    persist_segment(root, &segment_file, &segment)?;
    segments.push(Arc::new(segment));
    segment_files.push(segment_file);

    let mut latest_by_path = loaded.latest_by_path.clone();
    apply_segment_resolution(
        segments.last().expect("new segment was appended"),
        &mut latest_by_path,
    );

    if segments.len() >= REPO_INDEX_COMPACTION_SEGMENTS {
        let compacted = compact_segments(generation, &segments, &latest_by_path);
        let compacted_file = compacted_segment_file_name(generation);
        persist_segment(root, &compacted_file, &compacted)?;
        segments = vec![Arc::new(compacted)];
        segment_files = vec![compacted_file];
        latest_by_path.clear();
        apply_segment_resolution(
            segments.first().expect("compaction creates one segment"),
            &mut latest_by_path,
        );
    }

    let total_files = visible_file_count(&loaded.base, &latest_by_path);
    let overlay_files = latest_by_path
        .values()
        .filter(|owner| owner.is_some())
        .count();
    let manifest = RepoIndexRuntimeManifest {
        schema_version: REPO_INDEX_RUNTIME_SCHEMA_VERSION,
        generation,
        include_tests,
        total_files,
        overlay_files,
        segment_count: segments.len(),
        status: "ready".to_string(),
        recovered_from_generation: None,
        last_error: None,
    };
    let record = RepoIndexGenerationRecord {
        schema_version: REPO_INDEX_RUNTIME_SCHEMA_VERSION,
        generation,
        base_file: loaded.base_file.clone(),
        segment_files: segment_files.clone(),
        manifest: manifest.clone(),
    };
    persist_generation_record(root, &record)?;

    let runtime = RepoIndexRuntime {
        manifest: manifest.clone(),
        generation: Some(Arc::new(RepoIndexGeneration {
            base: Arc::clone(&loaded.base),
            base_file: loaded.base_file.clone(),
            segments,
            segment_files,
            latest_by_path,
        })),
    };
    validate_runtime(&runtime)?;
    publish_manifest(root, Some(&current.manifest), &manifest)?;
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
    {
        return Err(cache_error(format!(
            "generation record '{}' does not match its published manifest",
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
    for file_name in &record.segment_files {
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
        segments,
        segment_files: record.segment_files,
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

fn normalize_changed_paths(paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .map(|path| normalize_path(path))
        .filter(|path| !path.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn persist_segment(
    root: &Path,
    file_name: &str,
    segment: &RepoIndexSegment,
) -> Result<(), CovyError> {
    let encoded = wincode::serialize(segment)
        .map_err(|error| cache_error(format!("failed to encode repository segment: {error}")))?;
    write_atomic(repo_index_dir(root).join(file_name), &encoded)?;
    let decoded = wincode::deserialize::<RepoIndexSegment>(&encoded)
        .map_err(|error| cache_error(format!("failed to validate repository segment: {error}")))?;
    validate_segment(&decoded)
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
    use crate::update_repo_index;

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
}
