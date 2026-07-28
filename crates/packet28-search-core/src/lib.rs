//! Persistent literal/regex indexing and indexed search verification.
//!
//! The crate builds a repository-local index, validates every persisted layer
//! before publication, and verifies indexed candidates against source files.
//! Fallible operations return [`SearchError`], allowing callers to distinguish
//! unavailable indexes, invalid queries, corruption, and typed dependency
//! failures without parsing an `anyhow` report.
//!
//! # Examples
//!
//! ```
//! use packet28_search_core::RegexIndexRuntime;
//!
//! let runtime = RegexIndexRuntime::default();
//! assert!(!runtime.is_loaded());
//! ```

extern crate packet28_binary_codec as wincode;

mod error;
mod weights;

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use ignore::WalkBuilder;
use memmap2::Mmap;
use packet28_reducer_core::{
    infer_symbols_from_pattern, SearchEngineStats, SearchGroup, SearchMatch, SearchRequest,
    SearchResult,
};
use regex::{Regex, RegexBuilder};
use regex_syntax::hir::literal::{ExtractKind, Extractor, Seq};
use regex_syntax::hir::{Hir, HirKind};
use serde::{Deserialize, Serialize};

pub use crate::error::{Result, SearchError};
use crate::weights::{pair_weight, WEIGHT_TABLE_VERSION};

const REGEX_INDEX_SCHEMA_VERSION: u32 = 3;
const REGEX_DIR_NAME: &str = "regex-v1";
const MANIFEST_FILE_NAME: &str = "manifest.json";
const BASE_LOOKUP_FILE_NAME: &str = "base.lookup.dat";
const BASE_POSTINGS_FILE_NAME: &str = "base.postings.dat";
const BASE_DOCS_FILE_NAME: &str = "docs.dat";
const OVERLAY_LOOKUP_FILE_NAME: &str = "overlay.lookup.dat";
const OVERLAY_POSTINGS_FILE_NAME: &str = "overlay.postings.dat";
const OVERLAY_DOCS_FILE_NAME: &str = "overlay.docs.dat";
const OVERLAY_STATE_FILE_NAME: &str = "overlay.state.json";
const LOOKUP_ROW_BYTES: usize = 24;
const SHORT_GRAM_BYTES: usize = 2;
const MIN_GRAM_BYTES: usize = 3;
const MAX_GRAM_BYTES: usize = 24;
const MAX_LITERAL_COVER: usize = 8;
const MAX_INDEXED_FILE_BYTES: usize = 2 * 1024 * 1024;
const SEGMENT_DOC_BATCH_SIZE: usize = 256;
const SEGMENT_RECORD_BYTES: usize = 14;
const MAX_INDEX_VERIFY_CANDIDATES: usize = 1024;
const MAX_INDEX_VERIFY_NUMERATOR: usize = 1;
const MAX_INDEX_VERIFY_DENOMINATOR: usize = 2;
const POSITION_BUCKET_COUNT: usize = 16;

trait ResultContext<T> {
    fn context<C>(self, context: C) -> Result<T>
    where
        C: Into<String>;

    fn with_context<C, F>(self, context: F) -> Result<T>
    where
        C: Into<String>,
        F: FnOnce() -> C;
}

impl<T, E> ResultContext<T> for std::result::Result<T, E>
where
    E: Into<SearchError>,
{
    fn context<C>(self, context: C) -> Result<T>
    where
        C: Into<String>,
    {
        self.map_err(|source| source.into().context(context))
    }

    fn with_context<C, F>(self, context: F) -> Result<T>
    where
        C: Into<String>,
        F: FnOnce() -> C,
    {
        self.map_err(|source| source.into().context(context()))
    }
}

macro_rules! ensure_valid_index {
    ($condition:expr, $($message:tt)+) => {
        if !$condition {
            return Err(SearchError::corrupt(format!($($message)+)));
        }
    };
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
/// Persistent metadata describing the currently published regex index.
pub struct RegexIndexManifest {
    /// On-disk index schema version.
    pub schema_version: u32,
    /// Version of the gram weighting table used by the index.
    pub weight_table_version: u32,
    /// Monotonically increasing publication generation.
    pub generation: u64,
    /// Whether test files were included during the last full build.
    pub include_tests: bool,
    /// Persistent lifecycle status such as `building`, `ready`, or `corrupt`.
    pub status: String,
    /// Number of discovered files in the last full build.
    pub total_files: usize,
    /// Number of files represented in the base layer.
    pub indexed_files: usize,
    /// Number of files represented in the overlay layer.
    pub overlay_files: usize,
    /// Git commit associated with the base layer, when available.
    pub base_commit: Option<String>,
    /// Reason the index cannot currently serve queries.
    pub stale_reason: Option<String>,
    /// Unix timestamp at which the latest build started.
    pub last_build_started_at_unix: Option<u64>,
    /// Unix timestamp at which the latest build completed.
    pub last_build_completed_at_unix: Option<u64>,
    /// Most recent build, validation, or loading failure.
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct OverlayState {
    shadowed_paths: BTreeSet<String>,
    deleted_paths: BTreeSet<String>,
}

#[derive(
    Debug, Clone, Serialize, Deserialize, Default, wincode::SchemaRead, wincode::SchemaWrite,
)]
#[serde(default)]
struct DocRecord {
    doc_id: u32,
    path: String,
    size: u64,
    mtime_secs: u64,
    fingerprint: String,
}

#[derive(Debug, Clone, Default)]
/// An immutable, validated search index generation and its public manifest.
pub struct RegexIndexRuntime {
    /// Metadata for the loaded or unavailable generation.
    pub manifest: RegexIndexManifest,
    loaded: Option<Arc<LoadedIndex>>,
}

impl RegexIndexRuntime {
    /// Returns whether validated base and overlay layers are available.
    pub fn is_loaded(&self) -> bool {
        self.loaded.is_some()
    }
}

#[derive(Debug)]
struct LoadedIndex {
    base: LoadedLayer,
    overlay: LoadedLayer,
    overlay_state: OverlayState,
}

#[derive(Debug)]
struct LoadedLayer {
    docs: Vec<DocRecord>,
    doc_ids_by_path: HashMap<String, u32>,
    lookup: Option<Mmap>,
    postings: Option<Mmap>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SparseCandidate {
    hash: u64,
    score: u32,
    start: usize,
    end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SearchPlan {
    All,
    Literal(Vec<u8>),
    And(Vec<SearchPlan>),
    Or(Vec<SearchPlan>),
}

impl SearchPlan {
    fn kind_str(&self) -> &'static str {
        match self {
            Self::All => "prefiltered_all",
            Self::Literal(_) => "literal",
            Self::And(_) => "and",
            Self::Or(_) => "or",
        }
    }
}

#[derive(Clone)]
struct CompiledSearch {
    verifier: Verifier,
    plan: SearchPlan,
    plan_kind: String,
    planner_fallback: Option<String>,
    must_fallback_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct HeapItem {
    hash: u64,
    doc_id: u32,
    summary: PositionSummary,
    segment_idx: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PositionSummary {
    buckets: u8,
    repeated: bool,
}

impl PositionSummary {
    fn new(bucket: u8) -> Self {
        Self {
            buckets: ((bucket & 0x0f) << 4) | (bucket & 0x0f),
            repeated: false,
        }
    }

    fn first_bucket(self) -> u8 {
        self.buckets >> 4
    }

    fn last_bucket(self) -> u8 {
        self.buckets & 0x0f
    }

    fn repeated(self) -> bool {
        self.repeated
    }

    fn update(&mut self, bucket: u8) {
        let bucket = bucket & 0x0f;
        let first = self.first_bucket().min(bucket);
        let last = self.last_bucket().max(bucket);
        self.buckets = (first << 4) | last;
        self.repeated = true;
    }

    fn merge(&mut self, other: PositionSummary) {
        let first = self.first_bucket().min(other.first_bucket());
        let last = self.last_bucket().max(other.last_bucket());
        self.buckets = (first << 4) | last;
        self.repeated = true;
        if other.repeated {
            self.repeated = true;
        }
    }

    fn encode(self) -> [u8; 2] {
        [self.buckets, u8::from(self.repeated)]
    }

    fn decode(bytes: [u8; 2]) -> Self {
        Self {
            buckets: bytes[0],
            repeated: bytes[1] != 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PostingEntry {
    doc_id: u32,
    summary: PositionSummary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LookupPostingMeta {
    offset: u64,
    len: u32,
    doc_count: u32,
}

type PostingRow = (u64, u64, u32, u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LiteralWindow {
    earliest_bucket: u8,
    latest_bucket: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct IndexedGram {
    hash: u64,
    summary: PositionSummary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum LayerKind {
    Base,
    Overlay,
}

#[derive(Default)]
struct QueryCache {
    postings: HashMap<(LayerKind, u64), Option<Vec<PostingEntry>>>,
    literal_candidates: HashMap<Vec<u8>, BTreeSet<String>>,
    literal_hashes: HashMap<Vec<u8>, Vec<u64>>,
    literal_repeat_requirements: HashMap<Vec<u8>, HashMap<u64, bool>>,
    literal_windows: HashMap<(String, Vec<u8>), Option<LiteralWindow>>,
}

#[derive(Clone)]
enum Verifier {
    Regex {
        regex: Regex,
        whole_file_prefilter: bool,
    },
    FixedBytes {
        needle: Vec<u8>,
        case_insensitive: bool,
    },
}

/// Loads and validates the index beneath `root`.
///
/// Stale or corrupt artifacts are represented as an unloaded runtime whose
/// manifest records the reason. The `Result` shape is retained for source
/// compatibility and future load failures that cannot be represented as index
/// state.
///
/// # Errors
///
/// The current loader converts artifact validation failures into an unloaded
/// [`RegexIndexRuntime`]; it does not otherwise return an error.
pub fn load_runtime(root: &Path) -> Result<RegexIndexRuntime> {
    let mut manifest = load_manifest(root);
    if manifest.schema_version == 0 {
        return Ok(RegexIndexRuntime {
            manifest,
            loaded: None,
        });
    }
    if manifest.schema_version != REGEX_INDEX_SCHEMA_VERSION
        || manifest.weight_table_version != WEIGHT_TABLE_VERSION
    {
        let found_schema = manifest.schema_version;
        let found_weight = manifest.weight_table_version;
        mark_manifest_unloaded(
            &mut manifest,
            "stale",
            format!(
                "regex index weight/schema mismatch (found schema={}, weight={}, expected schema={}, weight={})",
                found_schema,
                found_weight,
                REGEX_INDEX_SCHEMA_VERSION,
                WEIGHT_TABLE_VERSION
            ),
        );
        return Ok(RegexIndexRuntime {
            manifest,
            loaded: None,
        });
    }
    if manifest.status != "ready" {
        return Ok(RegexIndexRuntime {
            manifest,
            loaded: None,
        });
    }
    if let Some(expected) = manifest.base_commit.as_deref() {
        if current_git_commit(root).as_deref() != Some(expected) {
            let expected_commit = expected.to_string();
            let current_commit =
                current_git_commit(root).unwrap_or_else(|| "<unknown>".to_string());
            mark_manifest_unloaded(
                &mut manifest,
                "stale",
                format!(
                    "regex index base commit changed (indexed={}, current={})",
                    expected_commit, current_commit
                ),
            );
            return Ok(RegexIndexRuntime {
                manifest,
                loaded: None,
            });
        }
    }
    let base = match load_layer(
        root,
        BASE_LOOKUP_FILE_NAME,
        BASE_POSTINGS_FILE_NAME,
        BASE_DOCS_FILE_NAME,
    )
    .context("failed to load base regex index layer")
    {
        Ok(base) => base,
        Err(err) => {
            mark_manifest_unloaded(&mut manifest, "corrupt", format!("{err:#}"));
            return Ok(RegexIndexRuntime {
                manifest,
                loaded: None,
            });
        }
    };
    let overlay = match load_layer(
        root,
        OVERLAY_LOOKUP_FILE_NAME,
        OVERLAY_POSTINGS_FILE_NAME,
        OVERLAY_DOCS_FILE_NAME,
    )
    .context("failed to load overlay regex index layer")
    {
        Ok(overlay) => overlay,
        Err(err) => {
            mark_manifest_unloaded(&mut manifest, "corrupt", format!("{err:#}"));
            return Ok(RegexIndexRuntime {
                manifest,
                loaded: None,
            });
        }
    };
    let overlay_state = load_overlay_state(root);
    Ok(RegexIndexRuntime {
        manifest,
        loaded: Some(Arc::new(LoadedIndex {
            base,
            overlay,
            overlay_state,
        })),
    })
}

/// Rebuilds and atomically publishes every searchable file beneath `root`.
///
/// # Errors
///
/// Returns [`SearchError::Io`], [`SearchError::BinaryEncode`],
/// [`SearchError::BinaryDecode`], or [`SearchError::Json`] (possibly wrapped in
/// [`SearchError::Context`]) when discovery, index construction, validation, or
/// publication fails. [`SearchError::FailureProvenance`] reports the rarer case
/// where both the build and recording its failure fail.
pub fn rebuild_full_index(root: &Path, include_tests: bool) -> Result<RegexIndexRuntime> {
    rebuild_full_index_with_progress(root, include_tests, |_, _| {})
}

/// Rebuilds the full index and reports `(indexed_files, total_files)` progress.
///
/// The callback is invoked before scanning and after each discovered file.
///
/// # Errors
///
/// Returns the same typed failures as [`rebuild_full_index`].
pub fn rebuild_full_index_with_progress<F>(
    root: &Path,
    include_tests: bool,
    mut on_progress: F,
) -> Result<RegexIndexRuntime>
where
    F: FnMut(usize, usize),
{
    let started = now_unix();
    let mut manifest = load_manifest(root);
    manifest.schema_version = REGEX_INDEX_SCHEMA_VERSION;
    manifest.weight_table_version = WEIGHT_TABLE_VERSION;
    manifest.include_tests = include_tests;
    manifest.status = "building".to_string();
    manifest.last_build_started_at_unix = Some(started);
    manifest.stale_reason = Some(format!(
        "full regex index rebuild started at {started} has not published a ready generation"
    ));
    manifest.last_error = None;
    save_manifest(root, &manifest)?;

    let build_result = (|| -> Result<_> {
        let docs = scan_documents_with_progress(root, &mut on_progress)?;
        let base_layer = build_layer(
            root,
            &docs,
            BASE_LOOKUP_FILE_NAME,
            BASE_POSTINGS_FILE_NAME,
            BASE_DOCS_FILE_NAME,
        )?;
        let overlay_docs = Vec::<IndexedDocument>::new();
        let overlay_layer = build_layer(
            root,
            &overlay_docs,
            OVERLAY_LOOKUP_FILE_NAME,
            OVERLAY_POSTINGS_FILE_NAME,
            OVERLAY_DOCS_FILE_NAME,
        )?;
        let overlay_state = OverlayState::default();
        save_overlay_state(root, &overlay_state)?;
        Ok((docs, base_layer, overlay_layer, overlay_state))
    })();
    let (docs, base_layer, overlay_layer, overlay_state) = match build_result {
        Ok(built) => built,
        Err(error) => return Err(record_index_build_failure(root, &mut manifest, error)),
    };

    manifest.generation = manifest.generation.saturating_add(1);
    manifest.status = "ready".to_string();
    manifest.total_files = docs.len();
    manifest.indexed_files = docs.len();
    manifest.overlay_files = 0;
    manifest.base_commit = current_git_commit(root);
    manifest.stale_reason = None;
    manifest.last_build_completed_at_unix = Some(now_unix());
    manifest.last_error = None;
    save_manifest(root, &manifest)?;

    Ok(RegexIndexRuntime {
        manifest,
        loaded: Some(Arc::new(LoadedIndex {
            base: base_layer,
            overlay: overlay_layer,
            overlay_state,
        })),
    })
}

/// Rebuilds the mutable overlay for `changed_paths`.
///
/// A missing current runtime or an empty change set intentionally triggers a
/// full rebuild to preserve the historical behavior.
///
/// # Errors
///
/// Returns [`SearchError::IndexNotLoaded`] when a supplied runtime has no
/// validated layers. Filesystem, codec, JSON, corruption, and failure-provenance
/// errors use the corresponding [`SearchError`] variants.
pub fn update_overlay_index(
    root: &Path,
    current: Option<&RegexIndexRuntime>,
    changed_paths: &[String],
) -> Result<RegexIndexRuntime> {
    if current.is_none() || changed_paths.is_empty() {
        return rebuild_full_index(root, true);
    }
    let current = current.expect("checked above");
    let loaded = current.loaded.as_ref().ok_or(SearchError::IndexNotLoaded)?;
    let mut overlay_state = loaded.overlay_state.clone();
    let normalized = normalize_paths(root, changed_paths);
    let mut overlay_by_path = HashMap::<String, IndexedDocument>::new();
    for doc in &loaded.overlay.docs {
        if overlay_state.deleted_paths.contains(&doc.path) {
            continue;
        }
        let full_path = root.join(&doc.path);
        if let Some(indexed) = index_document(root, &full_path)? {
            overlay_by_path.insert(doc.path.clone(), indexed);
        }
    }
    for path in normalized {
        overlay_state.shadowed_paths.insert(path.clone());
        let full_path = root.join(&path);
        if !full_path.exists() {
            overlay_state.deleted_paths.insert(path.clone());
            overlay_by_path.remove(&path);
            continue;
        }
        overlay_state.deleted_paths.remove(&path);
        if let Some(indexed) = index_document(root, &full_path)? {
            overlay_by_path.insert(path, indexed);
        }
    }
    let mut overlay_docs = overlay_by_path.into_values().collect::<Vec<_>>();
    overlay_docs.sort_by(|left, right| left.path.cmp(&right.path));
    for (idx, doc) in overlay_docs.iter_mut().enumerate() {
        doc.doc_id = idx as u32;
    }
    let mut manifest = load_manifest(root);
    manifest.status = "building".to_string();
    let started = now_unix();
    manifest.last_build_started_at_unix = Some(started);
    manifest.stale_reason = Some(format!(
        "regex index overlay update started at {started} has not published a ready generation"
    ));
    manifest.last_error = None;
    save_manifest(root, &manifest)?;

    let build_result = (|| -> Result<_> {
        let overlay_layer = build_layer(
            root,
            &overlay_docs,
            OVERLAY_LOOKUP_FILE_NAME,
            OVERLAY_POSTINGS_FILE_NAME,
            OVERLAY_DOCS_FILE_NAME,
        )?;
        let base_layer = load_layer(
            root,
            BASE_LOOKUP_FILE_NAME,
            BASE_POSTINGS_FILE_NAME,
            BASE_DOCS_FILE_NAME,
        )
        .context("failed to validate base layer before overlay publication")?;
        save_overlay_state(root, &overlay_state)?;
        Ok((base_layer, overlay_layer))
    })();
    let (base_layer, overlay_layer) = match build_result {
        Ok(built) => built,
        Err(error) => return Err(record_index_build_failure(root, &mut manifest, error)),
    };

    manifest.status = "ready".to_string();
    manifest.overlay_files = overlay_docs.len();
    manifest.stale_reason = None;
    manifest.last_build_completed_at_unix = Some(now_unix());
    manifest.last_error = None;
    save_manifest(root, &manifest)?;

    Ok(RegexIndexRuntime {
        manifest,
        loaded: Some(Arc::new(LoadedIndex {
            base: base_layer,
            overlay: overlay_layer,
            overlay_state,
        })),
    })
}

/// Removes the persisted regex index beneath `root`.
///
/// # Errors
///
/// Returns [`SearchError::Context`] with a nested [`SearchError::Io`] when the
/// index directory cannot be removed.
pub fn clear_index(root: &Path) -> Result<()> {
    let path = regex_index_dir(root);
    if path.exists() {
        fs::remove_dir_all(&path)
            .with_context(|| format!("failed to remove regex index dir '{}'", path.display()))?;
    }
    Ok(())
}

/// Returns why a request should use the legacy search engine, if applicable.
///
/// # Errors
///
/// Returns [`SearchError::InvalidRegexSyntax`] or [`SearchError::InvalidRegex`]
/// for invalid expressions, and a typed corruption or conversion failure if a
/// previously validated posting cannot be planned safely.
pub fn guarded_fallback_reason(
    root: &Path,
    runtime: &RegexIndexRuntime,
    request: &SearchRequest,
) -> Result<Option<String>> {
    if !runtime.is_loaded() || runtime.manifest.status != "ready" {
        let reason = runtime
            .manifest
            .stale_reason
            .clone()
            .or_else(|| runtime.manifest.last_error.clone())
            .unwrap_or_else(|| "regex search index is not ready".to_string());
        return Ok(Some(reason));
    }
    let loaded = runtime.loaded.as_ref().ok_or(SearchError::IndexNotLoaded)?;
    let compiled = compile_request(request, loaded.as_ref())?;
    if let Some(reason) = compiled.must_fallback_reason.clone() {
        return Ok(Some(reason));
    }
    if matches!(compiled.plan, SearchPlan::All) {
        return Ok(Some(compiled.planner_fallback.unwrap_or_else(|| {
            "planner could not derive a selective index plan".to_string()
        })));
    }
    let (resolved_paths, _) = resolve_requested_paths(root, &request.requested_paths);
    let requested_filter = requested_filter_set(&resolved_paths);
    let all_paths = all_indexed_paths(loaded.as_ref(), requested_filter.as_ref());
    let mut engine = SearchEngineStats {
        engine: "indexed_regex".to_string(),
        index_generation: Some(runtime.manifest.generation),
        base_commit: runtime.manifest.base_commit.clone(),
        plan_kind: Some(compiled.plan_kind.clone()),
        planner_fallback: compiled
            .must_fallback_reason
            .clone()
            .or(compiled.planner_fallback.clone()),
        stale_reason: runtime.manifest.stale_reason.clone(),
        candidates_examined: 0,
        candidate_files: 0,
        verified_files: 0,
        index_lookups: 0,
        postings_bytes_read: 0,
        fallback_reason: None,
    };
    let mut cache = QueryCache::default();
    let candidates = candidate_paths_for_plan(
        loaded.as_ref(),
        &compiled.plan,
        requested_filter.as_ref(),
        &all_paths,
        &mut cache,
        &mut engine,
    )?;
    let pruned_candidates =
        prune_candidates_with_positions(loaded.as_ref(), &compiled.plan, &candidates, &mut cache);
    if should_fallback_to_rg(pruned_candidates.len(), all_paths.len()) {
        return Ok(Some(format!(
            "candidate set remained too broad for indexed verification ({}/{} files)",
            pruned_candidates.len(),
            all_paths.len()
        )));
    }
    Ok(None)
}

/// Executes an indexed search and verifies candidate files against the request.
///
/// # Errors
///
/// Returns [`SearchError::IndexNotLoaded`] when `runtime` has no validated
/// generation, [`SearchError::EmptyQuery`] for a blank query, typed regex errors
/// for invalid expressions, [`SearchError::Io`] when a candidate cannot be read,
/// or a typed corruption/conversion error for an invalid posting.
pub fn indexed_search(
    root: &Path,
    runtime: &RegexIndexRuntime,
    request: &SearchRequest,
) -> Result<SearchResult> {
    let loaded = runtime.loaded.as_ref().ok_or(SearchError::IndexNotLoaded)?;
    let query = request.query.trim();
    if query.is_empty() {
        return Err(SearchError::EmptyQuery);
    }

    let (resolved_paths, mut diagnostics) = resolve_requested_paths(root, &request.requested_paths);
    let requested_filter = requested_filter_set(&resolved_paths);
    let compiled = compile_request(request, loaded.as_ref())?;
    let mut engine = SearchEngineStats {
        engine: "indexed_regex".to_string(),
        index_generation: Some(runtime.manifest.generation),
        base_commit: runtime.manifest.base_commit.clone(),
        plan_kind: Some(compiled.plan_kind.clone()),
        planner_fallback: compiled
            .must_fallback_reason
            .clone()
            .or(compiled.planner_fallback.clone()),
        stale_reason: runtime.manifest.stale_reason.clone(),
        candidates_examined: 0,
        candidate_files: 0,
        verified_files: 0,
        index_lookups: 0,
        postings_bytes_read: 0,
        fallback_reason: None,
    };
    let mut cache = QueryCache::default();

    let all_paths = all_indexed_paths(loaded.as_ref(), requested_filter.as_ref());
    if let Some(reason) = compiled
        .must_fallback_reason
        .clone()
        .or(compiled.planner_fallback.clone())
    {
        diagnostics.push(reason);
    }
    let candidate_paths = candidate_paths_for_plan(
        loaded.as_ref(),
        &compiled.plan,
        requested_filter.as_ref(),
        &all_paths,
        &mut cache,
        &mut engine,
    )?;
    let pruned_candidate_paths = prune_candidates_with_positions(
        loaded.as_ref(),
        &compiled.plan,
        &candidate_paths,
        &mut cache,
    );

    engine.candidates_examined = candidate_paths.len();
    engine.candidate_files = candidate_paths.len();
    engine.verified_files = pruned_candidate_paths.len();

    let mut groups = Vec::new();
    let mut total_match_count = 0usize;
    for path in &pruned_candidate_paths {
        let file_groups =
            verify_path(root, path, &compiled.verifier, request.max_matches_per_file)?;
        if file_groups.is_empty() {
            continue;
        }
        total_match_count = total_match_count.saturating_add(file_groups.len());
        let displayed = file_groups.iter().take(12).cloned().collect::<Vec<_>>();
        groups.push(SearchGroup {
            path: path.clone(),
            match_count: file_groups.len(),
            displayed_match_count: displayed.len(),
            truncated: file_groups.len() > displayed.len(),
            matches: displayed,
        });
    }

    let paths = groups
        .iter()
        .map(|group| group.path.clone())
        .collect::<Vec<_>>();
    let regions = groups
        .iter()
        .flat_map(|group| {
            group
                .matches
                .iter()
                .map(|item| format!("{}:{}-{}", item.path, item.line, item.line))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let max_total_matches = request.max_total_matches.unwrap_or(50).clamp(1, 200);
    let mut returned_matches = Vec::new();
    for group in &groups {
        for item in &group.matches {
            if returned_matches.len() >= max_total_matches {
                break;
            }
            returned_matches.push(item.clone());
        }
        if returned_matches.len() >= max_total_matches {
            break;
        }
    }
    let returned_match_count = returned_matches.len();
    let compact_preview = render_compact_preview(total_match_count, &groups);

    Ok(SearchResult {
        query: query.to_string(),
        requested_paths: request.requested_paths.clone(),
        resolved_paths,
        match_count: total_match_count,
        returned_match_count,
        truncated: total_match_count > returned_match_count,
        paths,
        regions,
        symbols: infer_symbols_from_pattern(query),
        groups,
        compact_preview,
        diagnostics,
        engine: Some(engine),
    })
}

fn build_verifier(request: &SearchRequest, query: &str) -> Result<Verifier> {
    if request.fixed_string && !request.whole_word && !matches!(request.case_sensitive, Some(false))
    {
        return Ok(Verifier::FixedBytes {
            needle: query.as_bytes().to_vec(),
            case_insensitive: matches!(request.case_sensitive, Some(false)),
        });
    }
    let pattern = if request.fixed_string {
        regex::escape(query)
    } else {
        query.to_string()
    };
    let pattern = if request.whole_word {
        format!(r"\b(?:{})\b", pattern)
    } else {
        pattern
    };
    let whole_file_prefilter = if request.fixed_string {
        true
    } else {
        let hir = regex_syntax::parse(query).map_err(|source| SearchError::InvalidRegexSyntax {
            query: query.to_string(),
            source: Box::new(source),
        })?;
        !hir_has_line_anchors(&hir)
    };
    let regex = RegexBuilder::new(&pattern)
        .case_insensitive(matches!(request.case_sensitive, Some(false)))
        .build()
        .map_err(|source| SearchError::InvalidRegex {
            query: query.to_string(),
            source: Box::new(source),
        })?;
    Ok(Verifier::Regex {
        regex,
        whole_file_prefilter,
    })
}

fn compile_request(request: &SearchRequest, loaded: &LoadedIndex) -> Result<CompiledSearch> {
    let query = request.query.trim();
    let verifier = build_verifier(request, query)?;
    let (plan, planner_fallback) = build_search_plan(request, query)?;
    let must_fallback_reason = classify_plan_fallback_reason(loaded, &plan);
    Ok(CompiledSearch {
        verifier,
        plan_kind: plan.kind_str().to_string(),
        plan,
        planner_fallback,
        must_fallback_reason,
    })
}

fn build_search_plan(request: &SearchRequest, query: &str) -> Result<(SearchPlan, Option<String>)> {
    if request.fixed_string {
        if matches!(request.case_sensitive, Some(false)) && !query.is_ascii() {
            return Ok((
                SearchPlan::All,
                Some(
                    "unicode ignore-case fixed-string queries use regex fallback instead of ASCII-only index normalization"
                        .to_string(),
                ),
            ));
        }
        let literal = normalize_for_index(query.as_bytes());
        if build_covering_hashes(&literal).is_empty() {
            return Ok((
                SearchPlan::All,
                Some(
                    "fixed string query is too short to derive a selective index plan".to_string(),
                ),
            ));
        }
        return Ok((SearchPlan::Literal(literal), None));
    }

    let hir = regex_syntax::parse(query).map_err(|source| SearchError::InvalidRegexSyntax {
        query: query.to_string(),
        source: Box::new(source),
    })?;
    let plan = normalize_plan(plan_from_hir(&hir));
    let planner_fallback = matches!(plan, SearchPlan::All).then(|| {
        "planner could not derive required literals; verifying all indexed candidates".to_string()
    });
    Ok((plan, planner_fallback))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlanStrength {
    Strong,
    Weak,
}

fn classify_plan_fallback_reason(loaded: &LoadedIndex, plan: &SearchPlan) -> Option<String> {
    match assess_plan_strength(loaded, plan) {
        Some(PlanStrength::Strong) => None,
        Some(PlanStrength::Weak) => Some(
            "planner derived only weak/common literals; routing broad regex to legacy_rg"
                .to_string(),
        ),
        None => Some("planner could not derive an index-safe branch set".to_string()),
    }
}

fn assess_plan_strength(loaded: &LoadedIndex, plan: &SearchPlan) -> Option<PlanStrength> {
    match plan {
        SearchPlan::All => None,
        SearchPlan::Literal(literal) => Some(literal_strength(loaded, literal)),
        SearchPlan::And(children) => {
            let mut saw_strong = false;
            let sibling_literal_count = children
                .iter()
                .filter(|child| matches!(child, SearchPlan::Literal(_)))
                .count();
            for child in children {
                match assess_plan_strength_with_siblings(loaded, child, sibling_literal_count) {
                    Some(PlanStrength::Strong) => saw_strong = true,
                    Some(PlanStrength::Weak) => {}
                    None => return None,
                }
            }
            Some(if saw_strong {
                PlanStrength::Strong
            } else {
                PlanStrength::Weak
            })
        }
        SearchPlan::Or(children) => {
            let mut saw_strong = false;
            for child in children {
                match assess_plan_strength(loaded, child) {
                    Some(PlanStrength::Strong) => saw_strong = true,
                    Some(PlanStrength::Weak) => {}
                    None => return None,
                }
            }
            let total_paths = all_indexed_paths(loaded, None).len().max(1);
            let estimated_candidates = estimate_plan_cardinality(loaded, plan, None, total_paths);
            if saw_strong || estimated_candidates.saturating_mul(2) <= total_paths {
                return Some(PlanStrength::Strong);
            }
            Some(PlanStrength::Weak)
        }
    }
}

fn assess_plan_strength_with_siblings(
    loaded: &LoadedIndex,
    plan: &SearchPlan,
    sibling_literal_count: usize,
) -> Option<PlanStrength> {
    match plan {
        SearchPlan::Literal(literal) => Some(literal_strength_with_siblings(
            loaded,
            literal,
            sibling_literal_count,
        )),
        _ => assess_plan_strength(loaded, plan),
    }
}

fn literal_strength(loaded: &LoadedIndex, literal: &[u8]) -> PlanStrength {
    literal_strength_with_siblings(loaded, literal, 0)
}

fn literal_strength_with_siblings(
    loaded: &LoadedIndex,
    literal: &[u8],
    sibling_literal_count: usize,
) -> PlanStrength {
    let total_paths = all_indexed_paths(loaded, None).len().max(1);
    let min_docs = select_covering_candidates(loaded, literal)
        .into_iter()
        .map(|candidate| {
            lookup_posting_count(&loaded.base, candidate.hash)
                .unwrap_or(0)
                .saturating_add(lookup_posting_count(&loaded.overlay, candidate.hash).unwrap_or(0))
                as usize
        })
        .min()
        .unwrap_or(total_paths);
    let literal_len = literal.len();
    if literal_len <= 3 && min_docs.saturating_mul(4) > total_paths {
        return PlanStrength::Weak;
    }
    if literal_len <= 4 && sibling_literal_count == 0 && min_docs.saturating_mul(3) > total_paths {
        return PlanStrength::Weak;
    }
    if literal_len >= 6 || min_docs <= 8 {
        return PlanStrength::Strong;
    }
    if sibling_literal_count > 0 && min_docs.saturating_mul(2) <= total_paths {
        return PlanStrength::Strong;
    }
    if min_docs.saturating_mul(8) <= total_paths {
        return PlanStrength::Strong;
    }
    PlanStrength::Weak
}

fn plan_from_hir(hir: &Hir) -> SearchPlan {
    match hir.kind() {
        HirKind::Empty | HirKind::Class(_) | HirKind::Look(_) => SearchPlan::All,
        HirKind::Literal(literal) => literal_plan_from_bytes(&literal.0),
        HirKind::Capture(capture) => combine_required_plan(plan_from_hir(&capture.sub), hir),
        HirKind::Concat(subs) => {
            combine_required_plan(normalize_and(subs.iter().map(plan_from_hir).collect()), hir)
        }
        HirKind::Alternation(subs) => {
            combine_required_plan(normalize_or(subs.iter().map(plan_from_hir).collect()), hir)
        }
        HirKind::Repetition(repetition) => plan_from_repetition(repetition, hir),
    }
}

fn plan_from_repetition(repetition: &regex_syntax::hir::Repetition, hir: &Hir) -> SearchPlan {
    if repetition.min == 0 {
        return SearchPlan::All;
    }
    let child_plan = plan_from_hir(&repetition.sub);
    let repeated_plan = if repetition.min > 1 {
        match &child_plan {
            SearchPlan::Literal(literal) if !literal.is_empty() => {
                let repeats_to_materialize = (repetition.min as usize)
                    .min((MAX_GRAM_BYTES / literal.len()).saturating_add(1));
                literal_plan_from_bytes(&literal.repeat(repeats_to_materialize))
            }
            _ => child_plan.clone(),
        }
    } else {
        child_plan
    };
    combine_required_plan(repeated_plan, hir)
}

fn combine_required_plan(structural: SearchPlan, hir: &Hir) -> SearchPlan {
    let extracted = plan_from_extractors(hir);
    match (normalize_plan(structural), normalize_plan(extracted)) {
        (SearchPlan::All, SearchPlan::All) => SearchPlan::All,
        (SearchPlan::All, other) | (other, SearchPlan::All) => other,
        (left, right) if left == right => left,
        (left, right) => normalize_and(vec![left, right]),
    }
}

fn plan_from_extractors(hir: &Hir) -> SearchPlan {
    let prefix = plan_from_extractor(hir, ExtractKind::Prefix);
    let suffix = plan_from_extractor(hir, ExtractKind::Suffix);
    combine_extractor_plans(prefix, suffix)
}

fn combine_extractor_plans(prefix: SearchPlan, suffix: SearchPlan) -> SearchPlan {
    match (prefix, suffix) {
        (SearchPlan::All, SearchPlan::All) => SearchPlan::All,
        (SearchPlan::All, other) | (other, SearchPlan::All) => other,
        (left, right) if left == right => left,
        (left, right) => normalize_and(vec![left, right]),
    }
}

fn plan_from_extractor(hir: &Hir, kind: ExtractKind) -> SearchPlan {
    let mut extractor = Extractor::new();
    extractor.limit_class(6).limit_repeat(8).limit_total(64);
    extractor.kind(kind.clone());
    let mut seq = extractor.extract(hir);
    if !seq.is_finite() || seq.is_empty() {
        return SearchPlan::All;
    }
    seq.minimize_by_preference();
    match kind {
        ExtractKind::Prefix => seq.keep_first_bytes(MAX_GRAM_BYTES),
        ExtractKind::Suffix => seq.keep_last_bytes(MAX_GRAM_BYTES),
        _ => return SearchPlan::All,
    }
    plan_from_literal_seq(&seq, kind)
}

fn plan_from_literal_seq(seq: &Seq, kind: ExtractKind) -> SearchPlan {
    let Some(literals) = seq.literals() else {
        return SearchPlan::All;
    };
    if let Some(common) = common_literal_from_seq(seq, kind) {
        return SearchPlan::Literal(common);
    }
    let mut normalized = Vec::<Vec<u8>>::new();
    for literal in literals {
        let bytes = normalize_for_index(literal.as_bytes());
        if is_poisonous_literal(&bytes) || build_covering_hashes(&bytes).is_empty() {
            continue;
        }
        if !normalized.iter().any(|existing| existing == &bytes) {
            normalized.push(bytes);
        }
    }
    match normalized.len() {
        0 => SearchPlan::All,
        1 => SearchPlan::Literal(normalized.into_iter().next().unwrap_or_default()),
        _ => SearchPlan::Or(normalized.into_iter().map(SearchPlan::Literal).collect()),
    }
}

fn common_literal_from_seq(seq: &Seq, kind: ExtractKind) -> Option<Vec<u8>> {
    let common = match kind {
        ExtractKind::Prefix => seq.longest_common_prefix(),
        ExtractKind::Suffix => seq.longest_common_suffix(),
        _ => None,
    }?;
    let bytes = normalize_for_index(common);
    if is_poisonous_literal(&bytes) || build_covering_hashes(&bytes).is_empty() {
        return None;
    }
    Some(bytes)
}

fn literal_plan_from_bytes(bytes: &[u8]) -> SearchPlan {
    let normalized = normalize_for_index(bytes);
    if is_poisonous_literal(&normalized) || build_covering_hashes(&normalized).is_empty() {
        SearchPlan::All
    } else {
        SearchPlan::Literal(normalized)
    }
}

fn is_poisonous_literal(bytes: &[u8]) -> bool {
    bytes.len() < SHORT_GRAM_BYTES
}

fn normalize_plan(plan: SearchPlan) -> SearchPlan {
    match plan {
        SearchPlan::And(children) => normalize_and(children),
        SearchPlan::Or(children) => normalize_or(children),
        other => other,
    }
}

fn normalize_and(children: Vec<SearchPlan>) -> SearchPlan {
    let mut normalized = Vec::new();
    for child in children {
        match normalize_plan(child) {
            SearchPlan::All => {}
            SearchPlan::And(nested) => normalized.extend(nested),
            other if !normalized.iter().any(|existing| existing == &other) => {
                normalized.push(other)
            }
            _ => {}
        }
    }
    if normalized.is_empty() {
        SearchPlan::All
    } else if normalized.len() == 1 {
        normalized.into_iter().next().unwrap_or(SearchPlan::All)
    } else {
        SearchPlan::And(normalized)
    }
}

fn normalize_or(children: Vec<SearchPlan>) -> SearchPlan {
    let mut normalized = Vec::new();
    for child in children {
        match normalize_plan(child) {
            SearchPlan::All => return SearchPlan::All,
            SearchPlan::Or(nested) => normalized.extend(nested),
            other if !normalized.iter().any(|existing| existing == &other) => {
                normalized.push(other)
            }
            _ => {}
        }
    }
    if normalized.is_empty() {
        SearchPlan::All
    } else if normalized.len() == 1 {
        normalized.into_iter().next().unwrap_or(SearchPlan::All)
    } else {
        SearchPlan::Or(normalized)
    }
}

fn candidate_paths_for_plan(
    loaded: &LoadedIndex,
    plan: &SearchPlan,
    requested_filter: Option<&BTreeSet<String>>,
    all_paths: &BTreeSet<String>,
    cache: &mut QueryCache,
    engine: &mut SearchEngineStats,
) -> Result<BTreeSet<String>> {
    match plan {
        SearchPlan::All => Ok(all_paths.clone()),
        SearchPlan::Literal(literal) => {
            if let Some(cached) = cache.literal_candidates.get(literal) {
                return Ok(cached.clone());
            }
            let candidates = select_covering_candidates(loaded, literal);
            if candidates.is_empty() {
                return Ok(all_paths.clone());
            }
            let mut literal_paths: Option<BTreeSet<String>> = None;
            let mut selected_hashes = Vec::new();
            let mut covered = vec![false; literal.len()];
            let mut previous_size = all_paths.len();
            for candidate in candidates {
                let current =
                    paths_for_hash(loaded, candidate.hash, requested_filter, cache, engine)?;
                literal_paths = Some(match literal_paths {
                    Some(existing) => existing.intersection(&current).cloned().collect(),
                    None => current,
                });
                selected_hashes.push(candidate.hash);
                let coverage_end = candidate.end.min(covered.len());
                for slot in covered.iter_mut().take(coverage_end).skip(candidate.start) {
                    *slot = true;
                }
                let covered_all = covered.iter().all(|covered_byte| *covered_byte);
                let current_size = literal_paths.as_ref().map_or(0, BTreeSet::len);
                let materially_reduced =
                    current_size.saturating_mul(10) < previous_size.saturating_mul(9);
                if should_stop_literal_refinement(
                    current_size,
                    all_paths.len(),
                    covered_all,
                    selected_hashes.len(),
                    materially_reduced,
                ) {
                    break;
                }
                previous_size = current_size.max(1);
            }
            let resolved = literal_paths.unwrap_or_else(|| all_paths.clone());
            cache
                .literal_candidates
                .insert(literal.clone(), resolved.clone());
            cache
                .literal_hashes
                .insert(literal.clone(), selected_hashes);
            cache
                .literal_repeat_requirements
                .insert(literal.clone(), repeat_requirements_for_literal(literal));
            Ok(resolved)
        }
        SearchPlan::And(children) => {
            let mut current: Option<BTreeSet<String>> = None;
            let mut ordered = children.iter().collect::<Vec<_>>();
            ordered.sort_by_key(|child| {
                estimate_plan_cardinality(loaded, child, requested_filter, all_paths.len())
            });
            for child in ordered {
                let child_paths = candidate_paths_for_plan(
                    loaded,
                    child,
                    requested_filter,
                    all_paths,
                    cache,
                    engine,
                )?;
                current = Some(match current {
                    Some(existing) => existing.intersection(&child_paths).cloned().collect(),
                    None => child_paths,
                });
            }
            Ok(current.unwrap_or_else(|| all_paths.clone()))
        }
        SearchPlan::Or(children) => {
            let mut union = BTreeSet::new();
            for child in children {
                union.extend(candidate_paths_for_plan(
                    loaded,
                    child,
                    requested_filter,
                    all_paths,
                    cache,
                    engine,
                )?);
            }
            Ok(union)
        }
    }
}

fn estimate_plan_cardinality(
    loaded: &LoadedIndex,
    plan: &SearchPlan,
    requested_filter: Option<&BTreeSet<String>>,
    all_path_count: usize,
) -> usize {
    match plan {
        SearchPlan::All => all_path_count,
        SearchPlan::Literal(literal) => {
            let hashes = select_covering_candidates(loaded, literal)
                .into_iter()
                .map(|candidate| candidate.hash)
                .collect::<Vec<_>>();
            if hashes.is_empty() {
                return all_path_count;
            }
            hashes
                .into_iter()
                .map(|hash| {
                    estimate_hash_cardinality(loaded, hash, requested_filter, all_path_count)
                })
                .min()
                .unwrap_or(all_path_count)
        }
        SearchPlan::And(children) => children
            .iter()
            .map(|child| estimate_plan_cardinality(loaded, child, requested_filter, all_path_count))
            .min()
            .unwrap_or(all_path_count),
        SearchPlan::Or(children) => children
            .iter()
            .map(|child| estimate_plan_cardinality(loaded, child, requested_filter, all_path_count))
            .sum::<usize>()
            .min(all_path_count),
    }
}

fn estimate_hash_cardinality(
    loaded: &LoadedIndex,
    hash: u64,
    requested_filter: Option<&BTreeSet<String>>,
    all_path_count: usize,
) -> usize {
    if let Some(filter) = requested_filter {
        let mut estimate = 0usize;
        if let Some(entries) = lookup_doc_ids_quiet(&loaded.base, hash) {
            for entry in entries {
                if let Some(doc) = loaded.base.docs.get(entry.doc_id as usize) {
                    if loaded.overlay_state.shadowed_paths.contains(&doc.path) {
                        continue;
                    }
                    if filter.contains(&doc.path) {
                        estimate = estimate.saturating_add(1);
                    }
                }
            }
        }
        if let Some(entries) = lookup_doc_ids_quiet(&loaded.overlay, hash) {
            for entry in entries {
                if let Some(doc) = loaded.overlay.docs.get(entry.doc_id as usize) {
                    if loaded.overlay_state.deleted_paths.contains(&doc.path) {
                        continue;
                    }
                    if filter.contains(&doc.path) {
                        estimate = estimate.saturating_add(1);
                    }
                }
            }
        }
        return estimate.min(all_path_count);
    }
    lookup_posting_count(&loaded.base, hash)
        .unwrap_or(0)
        .saturating_add(lookup_posting_count(&loaded.overlay, hash).unwrap_or(0)) as usize
}

fn select_covering_candidates(loaded: &LoadedIndex, literal: &[u8]) -> Vec<SparseCandidate> {
    let mut candidates = build_covering_candidates(literal);
    candidates.sort_by(|left, right| {
        let left_docs = lookup_posting_count(&loaded.base, left.hash)
            .unwrap_or(0)
            .saturating_add(lookup_posting_count(&loaded.overlay, left.hash).unwrap_or(0));
        let right_docs = lookup_posting_count(&loaded.base, right.hash)
            .unwrap_or(0)
            .saturating_add(lookup_posting_count(&loaded.overlay, right.hash).unwrap_or(0));
        left_docs
            .cmp(&right_docs)
            .then_with(|| (right.end - right.start).cmp(&(left.end - left.start)))
            .then_with(|| left.score.cmp(&right.score).reverse())
            .then_with(|| left.hash.cmp(&right.hash))
    });
    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();
    let mut covered = vec![false; literal.len()];
    for candidate in candidates {
        if !seen.insert(candidate.hash) {
            continue;
        }
        let adds_new_coverage = covered
            .iter()
            .enumerate()
            .skip(candidate.start)
            .take(candidate.end.saturating_sub(candidate.start))
            .any(|(_idx, slot)| !*slot);
        if adds_new_coverage || selected.len() < SHORT_GRAM_BYTES {
            let coverage_end = candidate.end.min(covered.len());
            for slot in covered.iter_mut().take(coverage_end).skip(candidate.start) {
                *slot = true;
            }
            selected.push(candidate);
        }
        if selected.len() >= MAX_LITERAL_COVER && covered.iter().all(|covered_byte| *covered_byte) {
            break;
        }
    }
    selected
}

fn paths_for_hash(
    loaded: &LoadedIndex,
    hash: u64,
    requested_filter: Option<&BTreeSet<String>>,
    cache: &mut QueryCache,
    engine: &mut SearchEngineStats,
) -> Result<BTreeSet<String>> {
    engine.index_lookups = engine.index_lookups.saturating_add(1);
    let mut paths = BTreeSet::new();

    if let Some(entries) =
        lookup_doc_ids_cached(&loaded.base, LayerKind::Base, hash, cache, engine)?
    {
        for entry in entries {
            if let Some(doc) = loaded.base.docs.get(entry.doc_id as usize) {
                if loaded.overlay_state.shadowed_paths.contains(&doc.path) {
                    continue;
                }
                if path_allowed(&doc.path, requested_filter) {
                    paths.insert(doc.path.clone());
                }
            }
        }
    }
    if let Some(entries) =
        lookup_doc_ids_cached(&loaded.overlay, LayerKind::Overlay, hash, cache, engine)?
    {
        for entry in entries {
            if let Some(doc) = loaded.overlay.docs.get(entry.doc_id as usize) {
                if loaded.overlay_state.deleted_paths.contains(&doc.path) {
                    continue;
                }
                if path_allowed(&doc.path, requested_filter) {
                    paths.insert(doc.path.clone());
                }
            }
        }
    }
    Ok(paths)
}

fn lookup_doc_ids_quiet(layer: &LoadedLayer, hash: u64) -> Option<Vec<PostingEntry>> {
    // INVARIANT: `load_layer` validates every lookup range and posting block before
    // constructing an immutable `LoadedLayer`, so decoding cannot fail here.
    let lookup = layer.lookup.as_ref()?;
    let postings = layer.postings.as_ref()?;
    let meta = lookup_posting_range(lookup, hash)?;
    let (start, end) = checked_posting_bounds(meta.offset, meta.len, postings.len()).ok()?;
    decode_postings(&postings[start..end]).ok()
}

fn lookup_doc_ids_cached(
    layer: &LoadedLayer,
    layer_kind: LayerKind,
    hash: u64,
    cache: &mut QueryCache,
    engine: &mut SearchEngineStats,
) -> Result<Option<Vec<PostingEntry>>> {
    if let Some(cached) = cache.postings.get(&(layer_kind, hash)) {
        return Ok(cached.clone());
    }
    let value = lookup_doc_ids(layer, hash, engine)?;
    cache.postings.insert((layer_kind, hash), value.clone());
    Ok(value)
}

fn lookup_posting_count(layer: &LoadedLayer, hash: u64) -> Option<u32> {
    let lookup = layer.lookup.as_ref()?;
    Some(lookup_posting_range(lookup, hash)?.doc_count)
}

fn lookup_doc_ids(
    layer: &LoadedLayer,
    hash: u64,
    engine: &mut SearchEngineStats,
) -> Result<Option<Vec<PostingEntry>>> {
    let Some(lookup) = layer.lookup.as_ref() else {
        return Ok(None);
    };
    let Some(postings) = layer.postings.as_ref() else {
        return Ok(None);
    };
    let Some(meta) = lookup_posting_range(lookup, hash) else {
        return Ok(None);
    };
    let (start, end) = checked_posting_bounds(meta.offset, meta.len, postings.len())
        .context("loaded regex index has an invalid posting range")?;
    engine.postings_bytes_read = engine
        .postings_bytes_read
        .saturating_add(u64::from(meta.len));
    Ok(Some(decode_postings(&postings[start..end])?))
}

fn verify_path(
    root: &Path,
    path: &str,
    verifier: &Verifier,
    max_matches_per_file: Option<usize>,
) -> Result<Vec<SearchMatch>> {
    let bytes = fs::read(root.join(path)).with_context(|| {
        format!(
            "failed to read candidate file '{}'",
            root.join(path).display()
        )
    })?;
    match verifier {
        Verifier::Regex {
            regex,
            whole_file_prefilter,
        } => {
            let text = String::from_utf8_lossy(&bytes);
            if *whole_file_prefilter && !regex.is_match(text.as_ref()) {
                return Ok(Vec::new());
            }
            collect_line_matches(path, &text, max_matches_per_file, |line| {
                regex.is_match(line)
            })
        }
        Verifier::FixedBytes {
            needle,
            case_insensitive,
        } => {
            if !contains_fixed_bytes(&bytes, needle, *case_insensitive) {
                return Ok(Vec::new());
            }
            let text = String::from_utf8_lossy(&bytes);
            let normalized_needle = case_insensitive.then(|| normalize_for_index(needle));
            collect_line_matches(path, &text, max_matches_per_file, |line| {
                if *case_insensitive {
                    normalize_for_index(line.as_bytes())
                        .windows(needle.len())
                        .any(|window| window == normalized_needle.as_deref().unwrap_or_default())
                } else {
                    line.as_bytes()
                        .windows(needle.len())
                        .any(|window| window == needle.as_slice())
                }
            })
        }
    }
}

fn collect_line_matches<F>(
    path: &str,
    text: &str,
    max_matches_per_file: Option<usize>,
    mut predicate: F,
) -> Result<Vec<SearchMatch>>
where
    F: FnMut(&str) -> bool,
{
    let mut matches = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if !predicate(line) {
            continue;
        }
        matches.push(SearchMatch {
            path: path.to_string(),
            line: idx + 1,
            text: line.to_string(),
        });
        if max_matches_per_file.is_some_and(|limit| matches.len() >= limit) {
            break;
        }
    }
    Ok(matches)
}

fn contains_fixed_bytes(bytes: &[u8], needle: &[u8], case_insensitive: bool) -> bool {
    if needle.is_empty() || bytes.len() < needle.len() {
        return false;
    }
    if case_insensitive {
        let haystack = normalize_for_index(bytes);
        let normalized_needle = normalize_for_index(needle);
        haystack
            .windows(normalized_needle.len())
            .any(|window| window == normalized_needle.as_slice())
    } else {
        bytes.windows(needle.len()).any(|window| window == needle)
    }
}

fn scan_documents_with_progress<F>(root: &Path, mut on_progress: F) -> Result<Vec<IndexedDocument>>
where
    F: FnMut(usize, usize),
{
    let mut docs = Vec::new();
    let paths = discover_document_paths(root)?;
    let total_files = paths.len();
    on_progress(0, total_files);
    for (idx, path) in paths.iter().enumerate() {
        if let Some(indexed) = index_document(root, path)? {
            docs.push(indexed);
        }
        on_progress(idx + 1, total_files);
    }
    docs.sort_by(|left, right| left.path.cmp(&right.path));
    for (idx, doc) in docs.iter_mut().enumerate() {
        doc.doc_id = idx as u32;
    }
    Ok(docs)
}

fn discover_document_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut walker = WalkBuilder::new(root);
    walker
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true);
    for entry in walker.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.into_path();
        if path.is_dir() {
            continue;
        }
        let Some(relative) = path.strip_prefix(root).ok() else {
            continue;
        };
        let normalized = relative.to_string_lossy().replace('\\', "/");
        if normalized.starts_with(".git/")
            || normalized.starts_with(".packet28/")
            || normalized.starts_with("target/")
            || normalized.starts_with("node_modules/")
        {
            continue;
        }
        out.push(path);
    }
    out.sort();
    Ok(out)
}

struct IndexedDocument {
    doc_id: u32,
    path: String,
    size: u64,
    mtime_secs: u64,
    fingerprint: String,
    grams: Vec<IndexedGram>,
}

fn index_document(root: &Path, path: &Path) -> Result<Option<IndexedDocument>> {
    let Some(relative) = path.strip_prefix(root).ok() else {
        return Ok(None);
    };
    let normalized = relative.to_string_lossy().replace('\\', "/");
    if normalized.starts_with(".git/")
        || normalized.starts_with(".packet28/")
        || normalized.starts_with("target/")
        || normalized.starts_with("node_modules/")
    {
        return Ok(None);
    }
    let metadata = fs::metadata(path)?;
    if metadata.len() as usize > MAX_INDEXED_FILE_BYTES {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    if bytes.is_empty() || bytes.contains(&0) {
        return Ok(None);
    }
    let grams = build_indexed_grams(&bytes);
    let fingerprint = blake3::hash(&bytes).to_hex().to_string();
    Ok(Some(IndexedDocument {
        doc_id: 0,
        path: normalized,
        size: metadata.len(),
        mtime_secs: mtime_secs(&metadata),
        fingerprint,
        grams,
    }))
}

fn build_layer(
    root: &Path,
    docs: &[IndexedDocument],
    lookup_name: &str,
    postings_name: &str,
    docs_name: &str,
) -> Result<LoadedLayer> {
    fs::create_dir_all(regex_index_dir(root))?;
    let segment_files = write_segment_files(root, lookup_name, docs)?;
    let (rows, postings) = merge_and_cleanup_segment_files(segment_files)?;
    let mut lookup = Vec::with_capacity(rows.len() * LOOKUP_ROW_BYTES);
    for (hash, offset, len, doc_count) in rows {
        lookup.extend_from_slice(&hash.to_le_bytes());
        lookup.extend_from_slice(&offset.to_le_bytes());
        lookup.extend_from_slice(&len.to_le_bytes());
        lookup.extend_from_slice(&doc_count.to_le_bytes());
    }
    let serialized_docs = docs
        .iter()
        .map(|doc| DocRecord {
            doc_id: doc.doc_id,
            path: doc.path.clone(),
            size: doc.size,
            mtime_secs: doc.mtime_secs,
            fingerprint: doc.fingerprint.clone(),
        })
        .collect::<Vec<_>>();
    write_atomic(regex_index_dir(root).join(lookup_name), &lookup)?;
    write_atomic(regex_index_dir(root).join(postings_name), &postings)?;
    write_atomic(
        regex_index_dir(root).join(docs_name),
        &wincode::serialize(&serialized_docs)?,
    )?;
    load_layer(root, lookup_name, postings_name, docs_name)
}

fn write_segment_files(
    root: &Path,
    lookup_name: &str,
    docs: &[IndexedDocument],
) -> Result<SegmentFiles> {
    let mut files = SegmentFiles::default();
    for (segment_idx, batch) in docs.chunks(SEGMENT_DOC_BATCH_SIZE).enumerate() {
        let mut pairs = Vec::<(u64, u32, PositionSummary)>::new();
        for doc in batch {
            for gram in &doc.grams {
                pairs.push((gram.hash, doc.doc_id, gram.summary));
            }
        }
        pairs.sort_unstable();
        pairs.dedup();
        let path = regex_index_dir(root).join(format!("{lookup_name}.{segment_idx:05}.segment"));
        write_segment_file(&path, &pairs)?;
        files.paths.push(path);
    }
    Ok(files)
}

#[derive(Debug, Default)]
struct SegmentFiles {
    paths: Vec<PathBuf>,
}

impl SegmentFiles {
    fn paths(&self) -> &[PathBuf] {
        &self.paths
    }
}

impl Drop for SegmentFiles {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = fs::remove_file(path);
        }
    }
}

fn write_segment_file(path: &Path, pairs: &[(u64, u32, PositionSummary)]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let mut file = File::create(&tmp)?;
    for (hash, doc_id, summary) in pairs {
        file.write_all(&hash.to_le_bytes())?;
        file.write_all(&doc_id.to_le_bytes())?;
        file.write_all(&summary.encode())?;
    }
    file.flush()?;
    drop(file);
    fs::rename(&tmp, path)?;
    Ok(())
}

fn merge_and_cleanup_segment_files(
    segment_files: SegmentFiles,
) -> Result<(Vec<PostingRow>, Vec<u8>)> {
    merge_segment_files(segment_files.paths())
}

fn merge_segment_files(segment_paths: &[PathBuf]) -> Result<(Vec<PostingRow>, Vec<u8>)> {
    let mut readers = Vec::new();
    let mut heap = BinaryHeap::<Reverse<HeapItem>>::new();
    for (segment_idx, path) in segment_paths.iter().enumerate() {
        let mut reader = BufReader::new(
            File::open(path)
                .with_context(|| format!("failed to open segment '{}'", path.display()))?,
        );
        if let Some((hash, doc_id, summary)) = read_segment_pair(&mut reader)
            .with_context(|| format!("failed to decode segment '{}'", path.display()))?
        {
            heap.push(Reverse(HeapItem {
                hash,
                doc_id,
                summary,
                segment_idx,
            }));
        }
        readers.push(reader);
    }

    let mut rows = Vec::<PostingRow>::new();
    let mut postings = Vec::new();
    let mut current_hash = None::<u64>;
    let mut current_docs = Vec::<PostingEntry>::new();

    while let Some(Reverse(item)) = heap.pop() {
        if current_hash != Some(item.hash) {
            flush_posting_group(&mut rows, &mut postings, current_hash, &current_docs);
            current_hash = Some(item.hash);
            current_docs.clear();
        }
        match current_docs.last_mut() {
            Some(last) if last.doc_id == item.doc_id => last.summary.merge(item.summary),
            _ => current_docs.push(PostingEntry {
                doc_id: item.doc_id,
                summary: item.summary,
            }),
        }
        let path = &segment_paths[item.segment_idx];
        if let Some((next_hash, next_doc_id, next_summary)) =
            read_segment_pair(&mut readers[item.segment_idx])
                .with_context(|| format!("failed to decode segment '{}'", path.display()))?
        {
            heap.push(Reverse(HeapItem {
                hash: next_hash,
                doc_id: next_doc_id,
                summary: next_summary,
                segment_idx: item.segment_idx,
            }));
        }
    }
    flush_posting_group(&mut rows, &mut postings, current_hash, &current_docs);
    Ok((rows, postings))
}

fn read_segment_pair(reader: &mut impl Read) -> Result<Option<(u64, u32, PositionSummary)>> {
    let mut record = [0u8; SEGMENT_RECORD_BYTES];
    let mut filled = 0usize;
    while filled < record.len() {
        match reader.read(&mut record[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => {
                return Err(SearchError::corrupt(format!(
                    "truncated segment record: expected {SEGMENT_RECORD_BYTES} bytes, found {filled}"
                )));
            }
            Ok(read) => filled += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error).context("failed while reading segment record"),
        }
    }
    Ok(Some((
        u64::from_le_bytes(record[0..8].try_into().expect("segment hash width")),
        u32::from_le_bytes(record[8..12].try_into().expect("segment doc id width")),
        PositionSummary::decode([record[12], record[13]]),
    )))
}

fn flush_posting_group(
    rows: &mut Vec<PostingRow>,
    postings: &mut Vec<u8>,
    current_hash: Option<u64>,
    current_docs: &[PostingEntry],
) {
    let Some(hash) = current_hash else {
        return;
    };
    if current_docs.is_empty() {
        return;
    }
    let offset = postings.len() as u64;
    let encoded = encode_postings(current_docs);
    postings.extend_from_slice(&encoded);
    rows.push((
        hash,
        offset,
        encoded.len() as u32,
        current_docs.len() as u32,
    ));
}

fn load_layer(
    root: &Path,
    lookup_name: &str,
    postings_name: &str,
    docs_name: &str,
) -> Result<LoadedLayer> {
    let dir = regex_index_dir(root);
    let docs_path = dir.join(docs_name);
    let lookup_path = dir.join(lookup_name);
    let postings_path = dir.join(postings_name);
    let docs_exists = docs_path.exists();
    let lookup_exists = lookup_path.exists();
    let postings_exists = postings_path.exists();
    let present_files = docs_exists as u8 + lookup_exists as u8 + postings_exists as u8;
    if present_files != 3 {
        return Err(SearchError::corrupt(format!(
            "incomplete regex index layer '{}': expected docs, lookup, and postings files; found {present_files}/3",
            docs_path.display()
        )));
    }
    let raw = fs::read(&docs_path)
        .with_context(|| format!("failed to read docs file '{}'", docs_path.display()))?;
    let docs = wincode::deserialize::<Vec<DocRecord>>(&raw)
        .with_context(|| format!("failed to decode docs file '{}'", docs_path.display()))?;
    let lookup = mmap_optional(&lookup_path)
        .with_context(|| format!("failed to map lookup file '{}'", lookup_path.display()))?;
    let postings = mmap_optional(&postings_path)
        .with_context(|| format!("failed to map postings file '{}'", postings_path.display()))?;
    validate_layer_files(
        &docs,
        lookup.as_deref().unwrap_or(&[]),
        postings.as_deref().unwrap_or(&[]),
        &docs_path,
        &lookup_path,
        &postings_path,
    )?;
    let doc_ids_by_path = docs
        .iter()
        .map(|doc| (doc.path.clone(), doc.doc_id))
        .collect::<HashMap<_, _>>();
    Ok(LoadedLayer {
        docs,
        doc_ids_by_path,
        lookup,
        postings,
    })
}

fn write_atomic(path: PathBuf, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let mut file = File::create(&tmp)?;
    file.write_all(bytes)?;
    file.flush()?;
    drop(file);
    fs::rename(&tmp, &path)?;
    Ok(())
}

fn mark_manifest_unloaded(manifest: &mut RegexIndexManifest, status: &str, reason: String) {
    manifest.status = status.to_string();
    manifest.stale_reason = Some(reason.clone());
    manifest.last_error = Some(reason);
}

fn record_index_build_failure(
    root: &Path,
    manifest: &mut RegexIndexManifest,
    error: SearchError,
) -> SearchError {
    mark_manifest_unloaded(manifest, "corrupt", format!("{error:#}"));
    if let Err(save_error) = save_manifest(root, manifest) {
        return SearchError::FailureProvenance {
            build: Box::new(error),
            persistence: Box::new(save_error),
        };
    }
    error
}

fn mmap_optional(path: &Path) -> Result<Option<Mmap>> {
    if !path.exists() || fs::metadata(path)?.len() == 0 {
        return Ok(None);
    }
    let file = File::open(path)?;
    let map = unsafe { Mmap::map(&file)? };
    Ok(Some(map))
}

fn validate_layer_files(
    docs: &[DocRecord],
    lookup: &[u8],
    postings: &[u8],
    docs_path: &Path,
    lookup_path: &Path,
    postings_path: &Path,
) -> Result<()> {
    let mut paths = BTreeSet::new();
    for (expected_id, doc) in docs.iter().enumerate() {
        let actual_id = usize::try_from(doc.doc_id).context("document id does not fit usize")?;
        ensure_valid_index!(
            actual_id == expected_id,
            "docs file '{}' has non-contiguous document id {} at row {expected_id}",
            docs_path.display(),
            doc.doc_id
        );
        ensure_valid_index!(
            paths.insert(doc.path.as_str()),
            "docs file '{}' contains duplicate path '{}'",
            docs_path.display(),
            doc.path
        );
    }

    let trailing = lookup.len() % LOOKUP_ROW_BYTES;
    ensure_valid_index!(
        trailing == 0,
        "lookup file '{}' has a partial trailing row: {trailing} of {LOOKUP_ROW_BYTES} bytes",
        lookup_path.display()
    );

    let postings_len =
        u64::try_from(postings.len()).context("postings file length does not fit u64")?;
    let mut previous_hash = None;
    let mut expected_offset = 0u64;
    for (row_index, row) in lookup.chunks_exact(LOOKUP_ROW_BYTES).enumerate() {
        let hash = u64::from_le_bytes(row[0..8].try_into().expect("lookup hash width"));
        let meta = LookupPostingMeta {
            offset: u64::from_le_bytes(row[8..16].try_into().expect("lookup offset width")),
            len: u32::from_le_bytes(row[16..20].try_into().expect("lookup length width")),
            doc_count: u32::from_le_bytes(
                row[20..24].try_into().expect("lookup document count width"),
            ),
        };
        if let Some(previous) = previous_hash {
            ensure_valid_index!(
                hash > previous,
                "lookup file '{}' row {row_index} has hash {hash} after {previous}; hashes must be strictly increasing",
                lookup_path.display()
            );
        }
        ensure_valid_index!(
            meta.len > 0 && meta.doc_count > 0,
            "lookup file '{}' row {row_index} has an empty posting block",
            lookup_path.display()
        );
        let (start, end) = checked_posting_bounds(meta.offset, meta.len, postings.len())
            .with_context(|| {
                format!(
                    "lookup file '{}' row {row_index} hash {hash} has invalid range into '{}'",
                    lookup_path.display(),
                    postings_path.display()
                )
            })?;
        ensure_valid_index!(
            meta.offset == expected_offset,
            "lookup file '{}' row {row_index} starts at {}, expected contiguous offset {expected_offset}",
            lookup_path.display(),
            meta.offset
        );
        let entries = decode_postings(&postings[start..end]).with_context(|| {
            format!(
                "lookup file '{}' row {row_index} hash {hash} references an invalid posting block in '{}'",
                lookup_path.display(),
                postings_path.display()
            )
        })?;
        let expected_doc_count =
            usize::try_from(meta.doc_count).context("posting document count does not fit usize")?;
        ensure_valid_index!(
            entries.len() == expected_doc_count,
            "lookup file '{}' row {row_index} declares {} documents but its posting block contains {}",
            lookup_path.display(),
            meta.doc_count,
            entries.len()
        );
        for entry in entries {
            ensure_valid_index!(
                usize::try_from(entry.doc_id)
                    .ok()
                    .is_some_and(|id| id < docs.len()),
                "lookup file '{}' row {row_index} references missing document id {}",
                lookup_path.display(),
                entry.doc_id
            );
        }
        previous_hash = Some(hash);
        expected_offset = u64::try_from(end).context("posting end does not fit u64")?;
    }
    ensure_valid_index!(
        expected_offset == postings_len,
        "postings file '{}' has {} unreferenced trailing bytes",
        postings_path.display(),
        postings_len.saturating_sub(expected_offset)
    );
    Ok(())
}

fn checked_posting_bounds(offset: u64, len: u32, postings_len: usize) -> Result<(usize, usize)> {
    let end = offset.checked_add(u64::from(len)).ok_or_else(|| {
        SearchError::corrupt(format!(
            "posting range offset {offset} + length {len} overflows u64"
        ))
    })?;
    let postings_len =
        u64::try_from(postings_len).context("postings file length does not fit u64")?;
    ensure_valid_index!(
        end <= postings_len,
        "posting range {offset}..{end} exceeds postings length {postings_len}"
    );
    Ok((
        usize::try_from(offset).context("posting offset does not fit usize")?,
        usize::try_from(end).context("posting end does not fit usize")?,
    ))
}

fn encode_postings(entries: &[PostingEntry]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    let mut previous = 0u32;
    for entry in entries {
        let delta = entry.doc_id.saturating_sub(previous);
        encode_varint(delta, &mut out);
        previous = entry.doc_id;
    }
    for entry in entries {
        out.extend_from_slice(&entry.summary.encode());
    }
    out
}

fn decode_postings(bytes: &[u8]) -> Result<Vec<PostingEntry>> {
    if bytes.len() < 4 {
        return Err(SearchError::corrupt("invalid posting block"));
    }
    let count = u32::from_le_bytes(bytes[0..4].try_into().expect("length checked")) as usize;
    let minimum_len = count
        .checked_mul(3)
        .and_then(|len| len.checked_add(4))
        .ok_or_else(|| {
            SearchError::corrupt("posting block document count overflows its encoded size")
        })?;
    ensure_valid_index!(
        bytes.len() >= minimum_len,
        "posting block declares {count} documents but is only {} bytes",
        bytes.len()
    );
    let mut doc_ids = Vec::with_capacity(count);
    let mut index = 4usize;
    let mut current = 0u32;
    for position in 0..count {
        let (delta, consumed) = decode_varint(&bytes[index..])?;
        ensure_valid_index!(
            position == 0 || delta > 0,
            "posting block document ids are not strictly increasing"
        );
        current = current
            .checked_add(delta)
            .ok_or_else(|| SearchError::corrupt("posting document id delta overflows u32"))?;
        doc_ids.push(current);
        index += consumed;
    }
    let summary_len = count
        .checked_mul(2)
        .ok_or_else(|| SearchError::corrupt("posting summary length overflows usize"))?;
    let summary_end = index
        .checked_add(summary_len)
        .ok_or_else(|| SearchError::corrupt("posting summary range overflows usize"))?;
    ensure_valid_index!(
        bytes.len() >= summary_end,
        "posting block missing positional summaries"
    );
    ensure_valid_index!(
        bytes.len() == summary_end,
        "posting block has {} trailing bytes",
        bytes.len() - summary_end
    );
    ensure_valid_index!(
        bytes[index..summary_end]
            .chunks_exact(2)
            .all(|summary| summary[1] <= 1),
        "posting block contains an invalid repeated-position flag"
    );
    Ok(doc_ids
        .into_iter()
        .enumerate()
        .map(|(offset, doc_id)| PostingEntry {
            doc_id,
            summary: PositionSummary::decode([
                bytes[index + (offset * 2)],
                bytes[index + (offset * 2) + 1],
            ]),
        })
        .collect())
}

fn encode_varint(mut value: u32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            return;
        }
    }
}

fn decode_varint(bytes: &[u8]) -> Result<(u32, usize)> {
    let mut result = 0u32;
    for (idx, byte) in bytes.iter().take(5).enumerate() {
        if idx == 4 && *byte > 0x0f {
            return Err(SearchError::corrupt("varint overflows u32"));
        }
        let value = u32::from(byte & 0x7f);
        result |= value << (idx * 7);
        if byte & 0x80 == 0 {
            return Ok((result, idx + 1));
        }
    }
    if bytes.len() >= 5 {
        return Err(SearchError::corrupt("varint overflows u32"));
    }
    Err(SearchError::corrupt("unterminated varint"))
}

fn lookup_posting_range(lookup: &[u8], hash: u64) -> Option<LookupPostingMeta> {
    debug_assert_eq!(
        lookup.len() % LOOKUP_ROW_BYTES,
        0,
        "lookup bytes must be validated before querying"
    );
    let rows = lookup.len() / LOOKUP_ROW_BYTES;
    let mut low = 0usize;
    let mut high = rows;
    while low < high {
        let mid = low + (high - low) / 2;
        let start = mid * LOOKUP_ROW_BYTES;
        let current = u64::from_le_bytes(lookup[start..start + 8].try_into().ok()?);
        if current == hash {
            let offset = u64::from_le_bytes(lookup[start + 8..start + 16].try_into().ok()?);
            let len = u32::from_le_bytes(lookup[start + 16..start + 20].try_into().ok()?);
            let doc_count = u32::from_le_bytes(lookup[start + 20..start + 24].try_into().ok()?);
            return Some(LookupPostingMeta {
                offset,
                len,
                doc_count,
            });
        }
        if current < hash {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    None
}

fn build_indexed_grams(bytes: &[u8]) -> Vec<IndexedGram> {
    let normalized = normalize_for_index(bytes);
    let mut by_hash = HashMap::<u64, PositionSummary>::new();
    for (start, gram) in contiguous_short_grams(&normalized) {
        add_indexed_gram(&mut by_hash, hash_bytes(&gram), start, normalized.len());
    }
    for (start, gram) in contiguous_trigrams(&normalized) {
        add_indexed_gram(&mut by_hash, hash_bytes(&gram), start, normalized.len());
    }
    for candidate in collect_sparse_candidates(&normalized) {
        add_indexed_gram(
            &mut by_hash,
            candidate.hash,
            candidate.start,
            normalized.len(),
        );
    }
    let mut grams = by_hash
        .into_iter()
        .map(|(hash, summary)| IndexedGram { hash, summary })
        .collect::<Vec<_>>();
    grams.sort_by_key(|gram| gram.hash);
    grams
}

fn add_indexed_gram(
    by_hash: &mut HashMap<u64, PositionSummary>,
    hash: u64,
    start: usize,
    byte_len: usize,
) {
    let bucket = bucket_for_offset(start, byte_len);
    by_hash
        .entry(hash)
        .and_modify(|summary| summary.update(bucket))
        .or_insert_with(|| PositionSummary::new(bucket));
}

fn build_covering_hashes(literal: &[u8]) -> Vec<u64> {
    build_covering_candidates(literal)
        .into_iter()
        .map(|candidate| candidate.hash)
        .collect()
}

fn build_covering_candidates(literal: &[u8]) -> Vec<SparseCandidate> {
    let normalized = normalize_for_index(literal);
    if normalized.len() == SHORT_GRAM_BYTES {
        return vec![SparseCandidate {
            hash: hash_bytes(&normalized),
            score: literal_score(&normalized),
            start: 0,
            end: normalized.len(),
        }];
    }
    if normalized.len() < MIN_GRAM_BYTES {
        return Vec::new();
    }
    let mut candidates = collect_sparse_candidates(&normalized);
    if candidates.is_empty() {
        candidates = contiguous_trigrams(&normalized)
            .into_iter()
            .map(|(start, gram)| SparseCandidate {
                hash: hash_bytes(&gram),
                score: literal_score(&gram),
                start,
                end: start + gram.len(),
            })
            .collect();
    }
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.hash.cmp(&right.hash))
    });
    candidates
        .into_iter()
        .fold(
            (BTreeSet::new(), Vec::new()),
            |(mut seen, mut items), candidate| {
                if seen.insert(candidate.hash) {
                    items.push(candidate);
                }
                (seen, items)
            },
        )
        .1
}

fn collect_sparse_candidates(bytes: &[u8]) -> Vec<SparseCandidate> {
    if bytes.len() < MIN_GRAM_BYTES + 1 {
        return Vec::new();
    }
    let weights = pair_weights_for_bytes(bytes);
    let prefixes = pair_weight_prefix_sums(&weights);
    let mut grams = Vec::new();
    for start in 0..=bytes.len() - MIN_GRAM_BYTES {
        let limit = (start + MAX_GRAM_BYTES).min(bytes.len());
        for end in (start + MIN_GRAM_BYTES + 1)..=limit {
            if !is_sparse_candidate_range(&weights, start, end) {
                continue;
            }
            grams.push(SparseCandidate {
                hash: hash_bytes(&bytes[start..end]),
                score: literal_score_range(&prefixes, start, end),
                start,
                end,
            });
        }
    }
    grams
}

fn contiguous_trigrams(bytes: &[u8]) -> Vec<(usize, Vec<u8>)> {
    if bytes.len() < MIN_GRAM_BYTES {
        return Vec::new();
    }
    bytes
        .windows(MIN_GRAM_BYTES)
        .enumerate()
        .map(|(start, window)| (start, window.to_vec()))
        .collect()
}

fn contiguous_short_grams(bytes: &[u8]) -> Vec<(usize, Vec<u8>)> {
    if bytes.len() < SHORT_GRAM_BYTES {
        return Vec::new();
    }
    bytes
        .windows(SHORT_GRAM_BYTES)
        .enumerate()
        .map(|(start, window)| (start, window.to_vec()))
        .collect()
}

fn pair_weights_for_bytes(bytes: &[u8]) -> Vec<u32> {
    bytes
        .windows(2)
        .map(|pair| pair_weight(pair[0], pair[1]))
        .collect()
}

fn pair_weight_prefix_sums(weights: &[u32]) -> Vec<u32> {
    let mut prefix = Vec::with_capacity(weights.len() + 1);
    prefix.push(0u32);
    for weight in weights {
        prefix.push(
            prefix
                .last()
                .copied()
                .unwrap_or_default()
                .saturating_add(*weight),
        );
    }
    prefix
}

fn is_sparse_candidate_range(weights: &[u32], start: usize, end: usize) -> bool {
    if end.saturating_sub(start) < MIN_GRAM_BYTES + 1 {
        return false;
    }
    let edge_left = weights[start];
    let edge_right = weights[end - 2];
    let interior_max = weights[start + 1..end - 2]
        .iter()
        .copied()
        .max()
        .unwrap_or(0);
    edge_left > interior_max && edge_right > interior_max
}

fn literal_score_range(prefixes: &[u32], start: usize, end: usize) -> u32 {
    let pair_score = prefixes[end - 1].saturating_sub(prefixes[start]);
    pair_score.saturating_add((end - start) as u32 * 32)
}

fn literal_score(bytes: &[u8]) -> u32 {
    let pair_score = bytes
        .windows(2)
        .map(|pair| pair_weight(pair[0], pair[1]))
        .sum::<u32>();
    pair_score.saturating_add((bytes.len() as u32) * 32)
}

fn normalize_for_index(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().map(|byte| byte.to_ascii_lowercase()).collect()
}

fn should_fallback_to_rg(candidate_count: usize, all_path_count: usize) -> bool {
    if candidate_count == 0 {
        return true;
    }
    if all_path_count == 0 {
        return false;
    }
    candidate_count > MAX_INDEX_VERIFY_CANDIDATES
        || candidate_count.saturating_mul(MAX_INDEX_VERIFY_DENOMINATOR)
            > all_path_count.saturating_mul(MAX_INDEX_VERIFY_NUMERATOR)
}

fn should_stop_literal_refinement(
    candidate_count: usize,
    all_path_count: usize,
    covered_all: bool,
    selected_count: usize,
    materially_reduced: bool,
) -> bool {
    if candidate_count == 0 || selected_count >= MAX_LITERAL_COVER {
        return true;
    }
    if !covered_all {
        return false;
    }
    if !materially_reduced && selected_count >= 3 {
        return true;
    }
    candidate_count <= 8
        || candidate_count.saturating_mul(8) <= all_path_count
        || selected_count >= 4
}

fn bucket_for_offset(offset: usize, byte_len: usize) -> u8 {
    if byte_len <= 1 {
        return 0;
    }
    ((offset.saturating_mul(POSITION_BUCKET_COUNT)) / byte_len)
        .min(POSITION_BUCKET_COUNT.saturating_sub(1)) as u8
}

fn prune_candidates_with_positions(
    loaded: &LoadedIndex,
    plan: &SearchPlan,
    candidates: &BTreeSet<String>,
    cache: &mut QueryCache,
) -> BTreeSet<String> {
    candidates
        .iter()
        .filter(|path| plan_window_for_path(loaded, path, plan, cache).is_some())
        .cloned()
        .collect()
}

fn plan_window_for_path(
    loaded: &LoadedIndex,
    path: &str,
    plan: &SearchPlan,
    cache: &mut QueryCache,
) -> Option<LiteralWindow> {
    match plan {
        SearchPlan::All => Some(LiteralWindow {
            earliest_bucket: 0,
            latest_bucket: (POSITION_BUCKET_COUNT as u8).saturating_sub(1),
        }),
        SearchPlan::Literal(literal) => literal_window_for_path(loaded, path, literal, cache),
        SearchPlan::Or(children) => {
            let mut earliest = u8::MAX;
            let mut latest = 0u8;
            let mut matched = false;
            for child in children {
                let Some(window) = plan_window_for_path(loaded, path, child, cache) else {
                    continue;
                };
                earliest = earliest.min(window.earliest_bucket);
                latest = latest.max(window.latest_bucket);
                matched = true;
            }
            matched.then_some(LiteralWindow {
                earliest_bucket: earliest,
                latest_bucket: latest,
            })
        }
        SearchPlan::And(children) => {
            let mut current: Option<LiteralWindow> = None;
            for child in children {
                let child_window = plan_window_for_path(loaded, path, child, cache)?;
                current = Some(match current {
                    None => child_window,
                    Some(existing) => {
                        if existing.earliest_bucket > child_window.latest_bucket {
                            return None;
                        }
                        LiteralWindow {
                            earliest_bucket: existing
                                .earliest_bucket
                                .min(child_window.earliest_bucket),
                            latest_bucket: existing.latest_bucket.max(child_window.latest_bucket),
                        }
                    }
                });
            }
            current
        }
    }
}

fn literal_window_for_path(
    loaded: &LoadedIndex,
    path: &str,
    literal: &[u8],
    cache: &mut QueryCache,
) -> Option<LiteralWindow> {
    let cache_key = (path.to_string(), literal.to_vec());
    if let Some(cached) = cache.literal_windows.get(&cache_key) {
        return *cached;
    }
    let hashes = cache
        .literal_hashes
        .get(literal)
        .cloned()
        .unwrap_or_else(|| {
            select_covering_candidates(loaded, literal)
                .into_iter()
                .map(|candidate| candidate.hash)
                .collect()
        });
    let repeat_requirements = cache
        .literal_repeat_requirements
        .get(literal)
        .cloned()
        .unwrap_or_else(|| repeat_requirements_for_literal(literal));
    if hashes.is_empty() {
        cache.literal_windows.insert(cache_key, None);
        return None;
    }
    let mut overlap_earliest = 0u8;
    let mut overlap_latest = (POSITION_BUCKET_COUNT as u8).saturating_sub(1);
    let mut union_earliest = u8::MAX;
    let mut union_latest = 0u8;
    for hash in hashes {
        let summary = lookup_summary_for_path(loaded, hash, path, cache)?;
        if repeat_requirements.get(&hash).copied().unwrap_or(false) && !summary.repeated() {
            cache.literal_windows.insert(cache_key, None);
            return None;
        }
        overlap_earliest = overlap_earliest.max(summary.first_bucket());
        overlap_latest = overlap_latest.min(summary.last_bucket());
        union_earliest = union_earliest.min(summary.first_bucket());
        union_latest = union_latest.max(summary.last_bucket());
    }
    let (earliest_bucket, latest_bucket) = if overlap_earliest <= overlap_latest {
        (overlap_earliest, overlap_latest)
    } else {
        (union_earliest, union_latest)
    };
    let window = Some(LiteralWindow {
        earliest_bucket,
        latest_bucket,
    });
    cache.literal_windows.insert(cache_key, window);
    window
}

fn repeat_requirements_for_literal(literal: &[u8]) -> HashMap<u64, bool> {
    let mut counts = HashMap::<u64, usize>::new();
    let normalized = normalize_for_index(literal);
    for gram in normalized.windows(SHORT_GRAM_BYTES) {
        *counts.entry(hash_bytes(gram)).or_insert(0) += 1;
    }
    for gram in normalized.windows(MIN_GRAM_BYTES) {
        *counts.entry(hash_bytes(gram)).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .filter_map(|(hash, count)| (count > 1).then_some((hash, true)))
        .collect()
}

fn hir_has_line_anchors(hir: &Hir) -> bool {
    match hir.kind() {
        HirKind::Look(look) => matches!(
            look,
            regex_syntax::hir::Look::Start
                | regex_syntax::hir::Look::End
                | regex_syntax::hir::Look::StartLF
                | regex_syntax::hir::Look::EndLF
                | regex_syntax::hir::Look::StartCRLF
                | regex_syntax::hir::Look::EndCRLF
        ),
        HirKind::Capture(capture) => hir_has_line_anchors(&capture.sub),
        HirKind::Concat(children) | HirKind::Alternation(children) => {
            children.iter().any(hir_has_line_anchors)
        }
        HirKind::Repetition(repetition) => hir_has_line_anchors(&repetition.sub),
        HirKind::Empty | HirKind::Literal(_) | HirKind::Class(_) => false,
    }
}

fn lookup_summary_for_path(
    loaded: &LoadedIndex,
    hash: u64,
    path: &str,
    cache: &mut QueryCache,
) -> Option<PositionSummary> {
    if loaded.overlay_state.deleted_paths.contains(path) {
        return None;
    }
    if let Some(doc_id) = loaded.overlay.doc_ids_by_path.get(path).copied() {
        return lookup_posting_entry(&loaded.overlay, LayerKind::Overlay, hash, doc_id, cache)
            .map(|entry| entry.summary);
    }
    if loaded.overlay_state.shadowed_paths.contains(path) {
        return None;
    }
    let doc_id = loaded.base.doc_ids_by_path.get(path).copied()?;
    lookup_posting_entry(&loaded.base, LayerKind::Base, hash, doc_id, cache)
        .map(|entry| entry.summary)
}

fn lookup_posting_entry(
    layer: &LoadedLayer,
    layer_kind: LayerKind,
    hash: u64,
    doc_id: u32,
    cache: &mut QueryCache,
) -> Option<PostingEntry> {
    let entries = cache
        .postings
        .entry((layer_kind, hash))
        .or_insert_with(|| lookup_doc_ids_quiet(layer, hash))
        .clone()?;
    entries.into_iter().find(|entry| entry.doc_id == doc_id)
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let digest = blake3::hash(bytes);
    u64::from_le_bytes(digest.as_bytes()[0..8].try_into().expect("slice length"))
}

fn regex_index_dir(root: &Path) -> PathBuf {
    root.join(".packet28").join("index").join(REGEX_DIR_NAME)
}

fn overlay_state_path(root: &Path) -> PathBuf {
    regex_index_dir(root).join(OVERLAY_STATE_FILE_NAME)
}

fn manifest_path(root: &Path) -> PathBuf {
    regex_index_dir(root).join(MANIFEST_FILE_NAME)
}

fn load_manifest(root: &Path) -> RegexIndexManifest {
    let path = manifest_path(root);
    let Ok(raw) = fs::read(path) else {
        return RegexIndexManifest::default();
    };
    serde_json::from_slice(&raw).unwrap_or_default()
}

fn save_manifest(root: &Path, manifest: &RegexIndexManifest) -> Result<()> {
    fs::create_dir_all(regex_index_dir(root))?;
    write_atomic(manifest_path(root), &serde_json::to_vec_pretty(manifest)?)
}

fn load_overlay_state(root: &Path) -> OverlayState {
    let Ok(raw) = fs::read(overlay_state_path(root)) else {
        return OverlayState::default();
    };
    serde_json::from_slice(&raw).unwrap_or_default()
}

fn save_overlay_state(root: &Path, overlay: &OverlayState) -> Result<()> {
    write_atomic(
        overlay_state_path(root),
        &serde_json::to_vec_pretty(overlay)?,
    )
}

fn current_git_commit(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn requested_filter_set(paths: &[String]) -> Option<BTreeSet<String>> {
    (!paths.is_empty()).then(|| paths.iter().cloned().collect())
}

fn all_indexed_paths(
    loaded: &LoadedIndex,
    requested_filter: Option<&BTreeSet<String>>,
) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for doc in &loaded.base.docs {
        if loaded.overlay_state.shadowed_paths.contains(&doc.path) {
            continue;
        }
        if path_allowed(&doc.path, requested_filter) {
            paths.insert(doc.path.clone());
        }
    }
    for doc in &loaded.overlay.docs {
        if loaded.overlay_state.deleted_paths.contains(&doc.path) {
            continue;
        }
        if path_allowed(&doc.path, requested_filter) {
            paths.insert(doc.path.clone());
        }
    }
    paths
}

fn path_allowed(path: &str, requested_filter: Option<&BTreeSet<String>>) -> bool {
    requested_filter.is_none_or(|filters| {
        filters
            .iter()
            .any(|filter| path == filter || path.starts_with(&format!("{filter}/")))
    })
}

fn normalize_paths(root: &Path, paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .map(|path| normalize_capture_path(root, path))
        .filter(|path| !path.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn normalize_capture_path(root: &Path, text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.contains('\n') {
        return String::new();
    }
    if trimmed == "." || trimmed == "./" {
        return String::new();
    }
    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        if let Ok(stripped) = path.strip_prefix(root) {
            return stripped
                .to_string_lossy()
                .replace('\\', "/")
                .trim_end_matches('/')
                .to_string();
        }
    }
    let normalized = trimmed
        .trim_start_matches("./")
        .trim_start_matches('/')
        .replace('\\', "/");
    normalized.trim_end_matches('/').to_string()
}

fn resolve_requested_paths(root: &Path, requested_paths: &[String]) -> (Vec<String>, Vec<String>) {
    let mut resolved = Vec::new();
    let mut diagnostics = Vec::new();
    let mut seen = BTreeSet::new();
    for original in requested_paths {
        let normalized = normalize_capture_path(root, original);
        if normalized.is_empty() {
            let trimmed = original.trim();
            if trimmed != "." && trimmed != "./" {
                diagnostics.push(format!("ignored invalid path input: {trimmed}"));
            }
            continue;
        }
        let direct = root.join(&normalized);
        let final_path = if direct.exists() {
            normalized
        } else if let Some(candidate) = resolve_capture_path_suffix(root, &normalized) {
            diagnostics.push(format!(
                "resolved missing path '{}' to '{}'",
                original.trim(),
                candidate
            ));
            candidate
        } else {
            diagnostics.push(format!(
                "path '{}' does not exist under daemon root {}",
                original.trim(),
                root.display()
            ));
            continue;
        };
        if seen.insert(final_path.clone()) {
            resolved.push(final_path);
        }
    }
    (resolved, diagnostics)
}

fn resolve_capture_path_suffix(root: &Path, needle: &str) -> Option<String> {
    let mut matches = BTreeSet::new();
    collect_suffix_matches(root, root, needle, &mut matches);
    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

fn collect_suffix_matches(
    root: &Path,
    current: &Path,
    needle: &str,
    matches: &mut BTreeSet<String>,
) {
    let Ok(entries) = fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_suffix_matches(root, &path, needle, matches);
            if matches.len() > 1 {
                return;
            }
            continue;
        }
        let Ok(stripped) = path.strip_prefix(root) else {
            continue;
        };
        let normalized = stripped.to_string_lossy().replace('\\', "/");
        if normalized == needle || normalized.ends_with(&format!("/{needle}")) {
            matches.insert(normalized);
            if matches.len() > 1 {
                return;
            }
        }
    }
}

fn render_compact_preview(total_match_count: usize, groups: &[SearchGroup]) -> String {
    if total_match_count == 0 {
        return "Search found 0 matches.".to_string();
    }
    let mut lines = vec![format!(
        "Search found {} matches in {} files.",
        total_match_count,
        groups.len()
    )];
    for group in groups.iter().take(12) {
        lines.push(format!("- {} ({})", group.path, group.match_count));
    }
    if groups.len() > 12 {
        lines.push(format!("+{} more files", groups.len() - 12));
    }
    lines.join("\n")
}

fn mtime_secs(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::Barrier;

    use super::*;

    fn build_fixture_index(root: &Path) -> RegexIndexRuntime {
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::write(
            root.join("src/lib.rs"),
            "pub struct Alpha;\npub fn alpha_service() {}\nconst ALPHA: &str = \"Alpha\";\n",
        )
        .unwrap();
        fs::write(
            root.join("src/nested/mod.rs"),
            "pub enum Beta { AlphaVariant }\nfn handle_value() { println!(\"beta\"); }\n",
        )
        .unwrap();
        rebuild_full_index(root, true).unwrap()
    }

    fn assert_parity(root: &Path, runtime: &RegexIndexRuntime, request: SearchRequest) {
        let indexed = indexed_search(root, runtime, &request).unwrap();
        let reducer = packet28_reducer_core::search(root, &request).unwrap();
        assert_eq!(
            indexed.match_count, reducer.match_count,
            "query={}",
            request.query
        );
        assert_eq!(indexed.paths, reducer.paths, "query={}", request.query);
        assert_eq!(indexed.regions, reducer.regions, "query={}", request.query);
    }

    fn build_all_hashes_for_test(bytes: &[u8]) -> Vec<u64> {
        build_indexed_grams(bytes)
            .into_iter()
            .map(|gram| gram.hash)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn encoded_segment_record(hash: u64, doc_id: u32, summary: PositionSummary) -> Vec<u8> {
        let mut record = Vec::with_capacity(SEGMENT_RECORD_BYTES);
        record.extend_from_slice(&hash.to_le_bytes());
        record.extend_from_slice(&doc_id.to_le_bytes());
        record.extend_from_slice(&summary.encode());
        record
    }

    fn corrupt_first_lookup_range(root: &Path, offset: u64, len: u32) {
        let path = regex_index_dir(root).join(BASE_LOOKUP_FILE_NAME);
        let mut lookup = fs::read(&path).unwrap();
        assert!(lookup.len() >= LOOKUP_ROW_BYTES);
        lookup[8..16].copy_from_slice(&offset.to_le_bytes());
        lookup[16..20].copy_from_slice(&len.to_le_bytes());
        fs::write(path, lookup).unwrap();
    }

    #[test]
    fn read_segment_pair_returns_none_at_a_clean_record_boundary() {
        let expected = (
            17,
            3,
            PositionSummary {
                buckets: 0x29,
                repeated: true,
            },
        );
        let mut reader = Cursor::new(encoded_segment_record(expected.0, expected.1, expected.2));

        assert_eq!(read_segment_pair(&mut reader).unwrap(), Some(expected));
        assert_eq!(read_segment_pair(&mut reader).unwrap(), None);
    }

    #[test]
    fn read_segment_pair_rejects_every_truncated_record_boundary() {
        for length in 1..SEGMENT_RECORD_BYTES {
            let mut reader = Cursor::new(vec![0u8; length]);
            let error = read_segment_pair(&mut reader).unwrap_err();

            assert!(
                error.to_string().contains(&format!(
                    "expected {SEGMENT_RECORD_BYTES} bytes, found {length}"
                )),
                "length={length}, error={error:#}"
            );
        }
    }

    #[test]
    fn merge_segment_files_cleans_temporary_segments_after_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corrupt.segment");
        fs::write(&path, vec![0u8; SEGMENT_RECORD_BYTES - 1]).unwrap();
        let files = SegmentFiles {
            paths: vec![path.clone()],
        };

        let error = merge_and_cleanup_segment_files(files).unwrap_err();

        assert!(
            !path.exists() && error.to_string().contains("failed to decode segment"),
            "path_exists={}, error={error:#}",
            path.exists()
        );
    }

    #[test]
    fn merge_segment_files_accepts_clean_eof_and_cleans_temporary_segments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("complete.segment");
        fs::write(&path, encoded_segment_record(7, 2, PositionSummary::new(4))).unwrap();
        let files = SegmentFiles {
            paths: vec![path.clone()],
        };

        let (rows, _) = merge_and_cleanup_segment_files(files).unwrap();

        assert_eq!(rows.len(), 1);
        assert!(!path.exists());
    }

    #[test]
    fn decode_postings_rejects_every_truncated_block_prefix() {
        let entries = [
            PostingEntry {
                doc_id: 0,
                summary: PositionSummary::new(0),
            },
            PostingEntry {
                doc_id: 127,
                summary: PositionSummary::new(7),
            },
            PostingEntry {
                doc_id: 128,
                summary: PositionSummary::new(15),
            },
        ];
        let encoded = encode_postings(&entries);

        for prefix_len in 0..encoded.len() {
            let result = decode_postings(&encoded[..prefix_len]);
            assert!(
                result.is_err(),
                "truncated posting prefix {prefix_len}/{} decoded successfully",
                encoded.len()
            );
        }
        assert_eq!(decode_postings(&encoded).unwrap(), entries);
    }

    #[test]
    fn decode_postings_rejects_impossible_count_before_allocating() {
        let encoded = u32::MAX.to_le_bytes();

        let error = decode_postings(&encoded).unwrap_err();

        assert!(
            error.to_string().contains("declares 4294967295 documents"),
            "{error:#}"
        );
    }

    #[test]
    fn decode_varint_rejects_values_larger_than_u32() {
        let error = decode_varint(&[0xff, 0xff, 0xff, 0xff, 0x10]).unwrap_err();

        assert!(error.to_string().contains("overflows u32"), "{error:#}");
    }

    #[test]
    fn checked_posting_bounds_matches_exhaustive_small_ranges() {
        for postings_len in 0usize..=16 {
            for offset in 0u64..=18 {
                for len in 0u32..=18 {
                    let expected = offset
                        .checked_add(u64::from(len))
                        .is_some_and(|end| end <= postings_len as u64);
                    assert_eq!(
                        checked_posting_bounds(offset, len, postings_len).is_ok(),
                        expected,
                        "offset={offset}, len={len}, postings_len={postings_len}"
                    );
                }
            }
        }
    }

    #[test]
    fn checked_posting_bounds_rejects_u64_overflow() {
        let error = checked_posting_bounds(u64::MAX, 1, usize::MAX).unwrap_err();

        assert!(error.to_string().contains("overflows u64"), "{error:#}");
    }

    #[test]
    fn sparse_grams_fall_back_to_trigrams() {
        let hashes = build_covering_hashes(b"Packet28");
        assert!(!hashes.is_empty());
    }

    #[test]
    fn build_all_hashes_cover_literal_coverings() {
        let hashes = build_all_hashes_for_test(b"pub(crate) fn handle_packet28_search(")
            .into_iter()
            .collect::<BTreeSet<_>>();
        for hash in build_covering_hashes(b"handle_packet28_search") {
            assert!(hashes.contains(&hash));
        }
        for hash in build_covering_hashes(b"fn") {
            assert!(hashes.contains(&hash));
        }
    }

    #[test]
    fn full_rebuild_and_overlay_search_shadow_base() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "pub struct Alpha;\n").unwrap();

        let runtime = rebuild_full_index(root, true).unwrap();
        let request = SearchRequest {
            query: "Alpha".to_string(),
            fixed_string: true,
            ..SearchRequest::default()
        };
        let result = indexed_search(root, &runtime, &request).unwrap();
        assert_eq!(result.match_count, 1);

        fs::write(root.join("src/lib.rs"), "pub struct Beta;\n").unwrap();
        let updated =
            update_overlay_index(root, Some(&runtime), &[String::from("src/lib.rs")]).unwrap();
        let result = indexed_search(root, &updated, &request).unwrap();
        assert_eq!(result.match_count, 0);
    }

    #[test]
    fn regex_search_builds_and_plan_for_concat_literals() {
        let (plan, fallback) = build_search_plan(
            &SearchRequest {
                query: "foo.*bar".to_string(),
                ..SearchRequest::default()
            },
            "foo.*bar",
        )
        .unwrap();
        assert_eq!(
            plan,
            SearchPlan::And(vec![
                SearchPlan::Literal(b"foo".to_vec()),
                SearchPlan::Literal(b"bar".to_vec())
            ])
        );
        assert_eq!(fallback, None);
    }

    #[test]
    fn regex_search_builds_or_plan_for_alternation() {
        let (plan, fallback) = build_search_plan(
            &SearchRequest {
                query: "(foo|bar)baz".to_string(),
                ..SearchRequest::default()
            },
            "(foo|bar)baz",
        )
        .unwrap();
        assert_eq!(
            plan,
            SearchPlan::And(vec![
                SearchPlan::Or(vec![
                    SearchPlan::Literal(b"foo".to_vec()),
                    SearchPlan::Literal(b"bar".to_vec())
                ]),
                SearchPlan::Literal(b"baz".to_vec()),
                SearchPlan::Or(vec![
                    SearchPlan::Literal(b"foobaz".to_vec()),
                    SearchPlan::Literal(b"barbaz".to_vec())
                ])
            ])
        );
        assert_eq!(fallback, None);
    }

    #[test]
    fn regex_search_keeps_short_alternation_branch_selective() {
        let (plan, fallback) = build_search_plan(
            &SearchRequest {
                query: r"pub\s+(?:fn|struct|enum)\s+[A-Za-z_][A-Za-z0-9_]*".to_string(),
                ..SearchRequest::default()
            },
            r"pub\s+(?:fn|struct|enum)\s+[A-Za-z_][A-Za-z0-9_]*",
        )
        .unwrap();
        assert_eq!(
            plan,
            SearchPlan::And(vec![
                SearchPlan::Literal(b"pub".to_vec()),
                SearchPlan::Or(vec![
                    SearchPlan::Literal(b"fn".to_vec()),
                    SearchPlan::Literal(b"struct".to_vec()),
                    SearchPlan::Literal(b"enum".to_vec())
                ])
            ])
        );
        assert_eq!(fallback, None);
    }

    #[test]
    fn regex_search_extracts_common_prefix_from_alternation_subtree() {
        let (plan, fallback) = build_search_plan(
            &SearchRequest {
                query: r"(packet28_search|packet28_read_regions)".to_string(),
                ..SearchRequest::default()
            },
            r"(packet28_search|packet28_read_regions)",
        )
        .unwrap();
        assert_eq!(
            plan,
            SearchPlan::And(vec![
                SearchPlan::Or(vec![
                    SearchPlan::Literal(b"packet28_search".to_vec()),
                    SearchPlan::Literal(b"packet28_read_regions".to_vec())
                ]),
                SearchPlan::Literal(b"packet28_".to_vec())
            ])
        );
        assert_eq!(fallback, None);
    }

    #[test]
    fn regex_search_materializes_bounded_repetition_literals() {
        let (plan, fallback) = build_search_plan(
            &SearchRequest {
                query: r"(ab){3}".to_string(),
                ..SearchRequest::default()
            },
            r"(ab){3}",
        )
        .unwrap();
        assert_eq!(plan, SearchPlan::Literal(b"ababab".to_vec()));
        assert_eq!(fallback, None);
    }

    #[test]
    fn lookup_rows_record_doc_counts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let runtime = build_fixture_index(root);
        let loaded = runtime.loaded.as_ref().expect("loaded index");
        let hash = build_covering_hashes(b"Alpha")
            .into_iter()
            .next()
            .expect("covering hash");
        let meta = lookup_posting_range(loaded.base.lookup.as_ref().expect("base lookup"), hash)
            .expect("lookup row");
        assert!(meta.doc_count >= 1);
    }

    #[test]
    fn weak_regex_plan_falls_back_to_all() {
        let (plan, fallback) = build_search_plan(
            &SearchRequest {
                query: ".+".to_string(),
                ..SearchRequest::default()
            },
            ".+",
        )
        .unwrap();
        assert_eq!(plan, SearchPlan::All);
        assert!(fallback.is_some());
    }

    #[test]
    fn load_runtime_marks_weight_mismatch_stale() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "pub struct Alpha;\n").unwrap();
        let runtime = rebuild_full_index(root, true).unwrap();
        let mut manifest = runtime.manifest.clone();
        manifest.weight_table_version = manifest.weight_table_version.saturating_sub(1);
        save_manifest(root, &manifest).unwrap();
        let loaded = load_runtime(root).unwrap();
        assert!(!loaded.is_loaded());
        assert_eq!(loaded.manifest.status, "stale");
        assert!(loaded.manifest.stale_reason.is_some());
    }

    #[test]
    fn load_runtime_marks_partial_layer_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "pub struct Alpha;\n").unwrap();
        let _runtime = rebuild_full_index(root, true).unwrap();
        fs::remove_file(regex_index_dir(root).join(BASE_POSTINGS_FILE_NAME)).unwrap();
        let loaded = load_runtime(root).unwrap();
        assert!(!loaded.is_loaded());
        assert_eq!(loaded.manifest.status, "corrupt");
        assert!(loaded.manifest.stale_reason.is_some());
    }

    #[test]
    fn load_runtime_rejects_every_partial_lookup_row_without_publication() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        drop(build_fixture_index(root));
        let lookup_path = regex_index_dir(root).join(BASE_LOOKUP_FILE_NAME);
        let original = fs::read(&lookup_path).unwrap();
        let complete_prefix_len = original.len() - LOOKUP_ROW_BYTES;

        for trailing in 1..LOOKUP_ROW_BYTES {
            fs::write(&lookup_path, &original[..complete_prefix_len + trailing]).unwrap();
            let runtime = load_runtime(root).unwrap();
            let reason = runtime.manifest.stale_reason.as_deref().unwrap_or_default();

            assert!(
                !runtime.is_loaded()
                    && runtime.manifest.status == "corrupt"
                    && runtime.manifest.last_error.as_deref() == Some(reason)
                    && reason.contains("failed to load base regex index layer")
                    && reason.contains(BASE_LOOKUP_FILE_NAME)
                    && reason.contains(&format!(
                        "partial trailing row: {trailing} of {LOOKUP_ROW_BYTES} bytes"
                    )),
                "trailing={trailing}, manifest={:?}",
                runtime.manifest
            );
        }
    }

    #[test]
    fn load_runtime_rejects_every_truncated_final_posting_block_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        drop(build_fixture_index(root));
        let lookup = fs::read(regex_index_dir(root).join(BASE_LOOKUP_FILE_NAME)).unwrap();
        let postings_path = regex_index_dir(root).join(BASE_POSTINGS_FILE_NAME);
        let postings = fs::read(&postings_path).unwrap();
        let final_row = &lookup[lookup.len() - LOOKUP_ROW_BYTES..];
        let offset = u64::from_le_bytes(final_row[8..16].try_into().unwrap()) as usize;
        let len = u32::from_le_bytes(final_row[16..20].try_into().unwrap()) as usize;
        assert_eq!(offset + len, postings.len());

        for prefix_len in 0..len {
            fs::write(&postings_path, &postings[..offset + prefix_len]).unwrap();
            let runtime = load_runtime(root).unwrap();

            assert!(
                !runtime.is_loaded()
                    && runtime.manifest.status == "corrupt"
                    && runtime
                        .manifest
                        .last_error
                        .as_deref()
                        .is_some_and(|error| error.contains("invalid range")),
                "prefix={prefix_len}/{len}, manifest={:?}",
                runtime.manifest
            );
        }
    }

    #[test]
    fn load_runtime_rejects_a_completely_missing_layer() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        drop(build_fixture_index(root));
        for name in [
            BASE_DOCS_FILE_NAME,
            BASE_LOOKUP_FILE_NAME,
            BASE_POSTINGS_FILE_NAME,
        ] {
            fs::remove_file(regex_index_dir(root).join(name)).unwrap();
        }

        let runtime = load_runtime(root).unwrap();

        assert!(!runtime.is_loaded());
        assert_eq!(runtime.manifest.status, "corrupt");
        assert!(
            runtime
                .manifest
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("found 0/3")),
            "{:?}",
            runtime.manifest
        );
    }

    #[test]
    fn load_runtime_preserves_an_unpublished_generation_marker() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let runtime = build_fixture_index(root);
        let mut manifest = runtime.manifest;
        manifest.status = "building".to_string();
        manifest.stale_reason = Some("interrupted overlay generation 17".to_string());
        save_manifest(root, &manifest).unwrap();

        let runtime = load_runtime(root).unwrap();

        assert!(!runtime.is_loaded());
        assert_eq!(
            runtime.manifest.stale_reason.as_deref(),
            Some("interrupted overlay generation 17")
        );
    }

    #[test]
    fn load_runtime_rejects_an_overflowing_posting_range_with_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        drop(build_fixture_index(root));
        corrupt_first_lookup_range(root, u64::MAX, 1);

        let runtime = load_runtime(root).unwrap();
        let reason = runtime.manifest.last_error.as_deref().unwrap_or_default();

        assert!(
            !runtime.is_loaded()
                && runtime.manifest.status == "corrupt"
                && runtime.manifest.stale_reason.as_deref() == Some(reason)
                && reason.contains("failed to load base regex index layer")
                && reason.contains(BASE_LOOKUP_FILE_NAME)
                && reason.contains(BASE_POSTINGS_FILE_NAME)
                && reason.contains("overflows u64"),
            "manifest={:?}",
            runtime.manifest
        );
    }

    #[test]
    fn load_runtime_rejects_an_out_of_bounds_posting_range() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        drop(build_fixture_index(root));
        let postings_len = fs::metadata(regex_index_dir(root).join(BASE_POSTINGS_FILE_NAME))
            .unwrap()
            .len();
        corrupt_first_lookup_range(root, postings_len, 1);

        let runtime = load_runtime(root).unwrap();

        assert!(!runtime.is_loaded());
        assert!(
            runtime
                .manifest
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("exceeds postings length")),
            "{:?}",
            runtime.manifest
        );
    }

    #[test]
    fn failed_overlay_validation_persists_provenance_without_publication() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let runtime = build_fixture_index(root);
        let lookup_path = regex_index_dir(root).join(BASE_LOOKUP_FILE_NAME);
        let lookup = fs::read(&lookup_path).unwrap();
        write_atomic(lookup_path, &lookup[..lookup.len() - 1]).unwrap();
        fs::write(root.join("src/lib.rs"), "pub struct Replacement;\n").unwrap();

        let error =
            update_overlay_index(root, Some(&runtime), &[String::from("src/lib.rs")]).unwrap_err();
        let manifest = load_manifest(root);

        assert!(
            manifest.status == "corrupt"
                && manifest.last_error.as_deref().is_some_and(|reason| {
                    reason.contains("failed to validate base layer before overlay publication")
                        && reason.contains("partial trailing row")
                })
                && !load_runtime(root).unwrap().is_loaded(),
            "error={error:#}, manifest={manifest:?}"
        );
    }

    #[test]
    fn concurrent_readers_keep_the_published_generation_while_loaders_reject_a_building_one() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let runtime = build_fixture_index(root);
        let mut manifest = runtime.manifest.clone();
        manifest.status = "building".to_string();
        manifest.stale_reason = Some("generation replacement in progress".to_string());
        save_manifest(root, &manifest).unwrap();
        let lookup_path = regex_index_dir(root).join(BASE_LOOKUP_FILE_NAME);
        let lookup = fs::read(&lookup_path).unwrap();
        let truncated_lookup = lookup[..lookup.len() - 1].to_vec();
        let request = SearchRequest {
            query: "Alpha".to_string(),
            fixed_string: true,
            ..SearchRequest::default()
        };
        let barrier = Arc::new(Barrier::new(10));

        std::thread::scope(|scope| {
            let query_handles = (0..4)
                .map(|_| {
                    let barrier = Arc::clone(&barrier);
                    let request = request.clone();
                    let runtime = &runtime;
                    scope.spawn(move || {
                        barrier.wait();
                        indexed_search(root, runtime, &request).map(|result| result.match_count)
                    })
                })
                .collect::<Vec<_>>();
            let load_handles = (0..4)
                .map(|_| {
                    let barrier = Arc::clone(&barrier);
                    scope.spawn(move || {
                        barrier.wait();
                        load_runtime(root).map(|runtime| runtime.is_loaded())
                    })
                })
                .collect::<Vec<_>>();
            let writer_barrier = Arc::clone(&barrier);
            let writer = scope.spawn(move || {
                writer_barrier.wait();
                write_atomic(lookup_path, &truncated_lookup)
            });

            barrier.wait();
            writer.join().unwrap().unwrap();
            for handle in query_handles {
                assert!(handle.join().unwrap().unwrap() > 0);
            }
            for handle in load_handles {
                assert!(!handle.join().unwrap().unwrap());
            }
        });
    }

    #[test]
    fn guarded_fallback_triggers_for_broad_candidate_sets() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        for idx in 0..128 {
            fs::write(
                root.join("src").join(format!("item_{idx}.rs")),
                format!("pub fn item_{idx}() {{}}\n"),
            )
            .unwrap();
        }
        let runtime = rebuild_full_index(root, true).unwrap();
        let request = SearchRequest {
            query: r"pub\s+fn\s+[A-Za-z_][A-Za-z0-9_]*".to_string(),
            ..SearchRequest::default()
        };
        let reason = guarded_fallback_reason(root, &runtime, &request).unwrap();
        assert!(reason.is_some());
    }

    #[test]
    fn guarded_fallback_allows_bounded_alternation_with_weak_branches() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        for idx in 0..128 {
            let content = match idx {
                0..=9 => format!("pub fn item_{idx}() {{ hook(); }}\n"),
                10..=19 => format!("pub fn item_{idx}() {{ mcp(); }}\n"),
                20..=24 => format!("pub fn item_{idx}() {{ tool_use(); }}\n"),
                _ => format!("pub fn item_{idx}() {{ filler_{idx}(); }}\n"),
            };
            fs::write(root.join("src").join(format!("item_{idx}.rs")), content).unwrap();
        }
        let runtime = rebuild_full_index(root, true).unwrap();
        let request = SearchRequest {
            query: "hook|mcp|tool_use".to_string(),
            ..SearchRequest::default()
        };

        let reason = guarded_fallback_reason(root, &runtime, &request).unwrap();
        assert_eq!(reason, None);

        let result = indexed_search(root, &runtime, &request).unwrap();
        assert_eq!(result.match_count, 25);
        assert_eq!(
            result.engine.as_ref().map(|engine| engine.engine.as_str()),
            Some("indexed_regex")
        );
    }

    #[test]
    fn dot_requested_path_means_repo_root_for_indexed_search() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn sample() { tool_use(); }\n").unwrap();
        for idx in 0..16 {
            fs::write(
                root.join("src").join(format!("filler_{idx}.rs")),
                format!("pub fn filler_{idx}() {{}}\n"),
            )
            .unwrap();
        }
        let runtime = rebuild_full_index(root, true).unwrap();
        let request = SearchRequest {
            query: "tool_use".to_string(),
            requested_paths: vec![".".to_string()],
            ..SearchRequest::default()
        };

        let reason = guarded_fallback_reason(root, &runtime, &request).unwrap();
        assert_eq!(reason, None);

        let result = indexed_search(root, &runtime, &request).unwrap();
        assert_eq!(result.match_count, 1);
        assert!(result.diagnostics.is_empty());
        assert_eq!(
            result.engine.as_ref().map(|engine| engine.engine.as_str()),
            Some("indexed_regex")
        );
    }

    #[test]
    fn guarded_fallback_triggers_when_query_hits_only_skipped_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "pub struct Alpha;\n").unwrap();
        let large = format!(
            "{}needle_only_in_large_file\n",
            "x".repeat(MAX_INDEXED_FILE_BYTES + 32)
        );
        fs::write(root.join("src/large.txt"), large).unwrap();

        let runtime = rebuild_full_index(root, true).unwrap();
        let request = SearchRequest {
            query: "needle_only_in_large_file".to_string(),
            fixed_string: true,
            ..SearchRequest::default()
        };
        let reason = guarded_fallback_reason(root, &runtime, &request).unwrap();
        assert!(reason.is_some());
    }

    #[test]
    fn positional_pruning_respects_literal_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/good.rs"),
            "fn sample() { let _ = foo(); bar(); }\n",
        )
        .unwrap();
        fs::write(
            root.join("src/bad.rs"),
            "fn sample() { let _ = bar(); foo(); }\n",
        )
        .unwrap();
        let runtime = rebuild_full_index(root, true).unwrap();
        let request = SearchRequest {
            query: "foo.*bar".to_string(),
            ..SearchRequest::default()
        };
        let result = indexed_search(root, &runtime, &request).unwrap();
        assert_eq!(result.paths, vec!["src/good.rs".to_string()]);
    }

    #[test]
    fn indexed_search_matches_directory_filters_with_trailing_slash() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let runtime = build_fixture_index(root);
        let request = SearchRequest {
            query: "AlphaVariant".to_string(),
            fixed_string: true,
            requested_paths: vec!["src/nested/".to_string()],
            ..SearchRequest::default()
        };
        let result = indexed_search(root, &runtime, &request).unwrap();
        assert_eq!(result.paths, vec!["src/nested/mod.rs".to_string()]);
    }

    #[test]
    fn indexed_search_matches_anchored_line_start_regexes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/main.rs"),
            "fn build() {\n    SearchRequest {\n        query: pattern,\n    };\n}\n",
        )
        .unwrap();

        let runtime = rebuild_full_index(root, true).unwrap();
        let request = SearchRequest {
            query: r"^\s*SearchRequest\s*\{".to_string(),
            ..SearchRequest::default()
        };
        let result = indexed_search(root, &runtime, &request).unwrap();
        assert_eq!(result.match_count, 1);
        assert_eq!(result.paths, vec!["src/main.rs".to_string()]);
        assert_eq!(result.groups[0].matches[0].line, 2);
    }

    #[test]
    fn regex_verifier_disables_whole_file_prefilter_for_anchored_queries() {
        let anchored = build_verifier(&SearchRequest::default(), r"^\s*SearchRequest\s*\{")
            .expect("anchored verifier");
        let plain = build_verifier(&SearchRequest::default(), r"handle_packet28_search")
            .expect("plain verifier");

        match anchored {
            Verifier::Regex {
                whole_file_prefilter,
                ..
            } => assert!(!whole_file_prefilter),
            _ => panic!("expected regex verifier"),
        }
        match plain {
            Verifier::Regex {
                whole_file_prefilter,
                ..
            } => assert!(whole_file_prefilter),
            _ => panic!("expected regex verifier"),
        }
    }

    #[test]
    fn literal_candidate_planning_caches_selected_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let runtime = build_fixture_index(root);
        let loaded = runtime.loaded.as_ref().expect("loaded index");
        let all_paths = all_indexed_paths(loaded.as_ref(), None);
        let mut cache = QueryCache::default();
        let mut engine = SearchEngineStats::default();
        let literal = b"alpha_service".to_vec();

        let paths = candidate_paths_for_plan(
            loaded.as_ref(),
            &SearchPlan::Literal(literal.clone()),
            None,
            &all_paths,
            &mut cache,
            &mut engine,
        )
        .expect("candidate paths");

        assert_eq!(paths, BTreeSet::from(["src/lib.rs".to_string()]));
        assert!(cache.literal_hashes.contains_key(&literal));
        assert!(!cache.literal_hashes[&literal].is_empty());
    }

    #[test]
    fn indexed_search_handles_non_ascii_ignore_case_fixed_queries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "const CAFE: &str = \"café\";\n").unwrap();

        let runtime = rebuild_full_index(root, true).unwrap();
        let request = SearchRequest {
            query: "CAFÉ".to_string(),
            fixed_string: true,
            case_sensitive: Some(false),
            ..SearchRequest::default()
        };
        let result = indexed_search(root, &runtime, &request).unwrap();
        assert_eq!(result.match_count, 1);
        assert_eq!(result.paths, vec!["src/lib.rs".to_string()]);
    }

    #[test]
    fn indexed_search_matches_reducer_for_common_queries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let runtime = build_fixture_index(root);

        let requests = vec![
            SearchRequest {
                query: "Alpha".to_string(),
                fixed_string: true,
                ..SearchRequest::default()
            },
            SearchRequest {
                query: "alpha".to_string(),
                fixed_string: true,
                case_sensitive: Some(false),
                ..SearchRequest::default()
            },
            SearchRequest {
                query: r"Alpha|Beta".to_string(),
                ..SearchRequest::default()
            },
            SearchRequest {
                query: "alpha_service".to_string(),
                fixed_string: true,
                whole_word: true,
                ..SearchRequest::default()
            },
            SearchRequest {
                query: "AlphaVariant".to_string(),
                fixed_string: true,
                requested_paths: vec!["src/nested".to_string()],
                ..SearchRequest::default()
            },
        ];

        for request in requests {
            assert_parity(root, &runtime, request);
        }
    }
}
