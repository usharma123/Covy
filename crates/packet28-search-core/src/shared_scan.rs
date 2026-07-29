//! Experimental borrowed-content rebuild session for daemon composition.
//!
//! This module is available only with the non-default
//! `shared-repository-scan` feature. It retains regex-specific filtering,
//! derived grams, writer locking, artifact validation, and manifest ownership
//! while allowing a coordinator to lend each file buffer.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use super::*;

/// Maximum file size consumed by the regex full-index builder.
pub const MAX_SHARED_SCAN_CONTENT_BYTES: usize = MAX_INDEXED_FILE_BYTES;

/// Content digests used to prove byte-for-byte base-layer parity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexIndexContentDigests {
    /// Digest of the fixed-width lookup table.
    pub lookup: String,
    /// Digest of the postings table.
    pub postings: String,
    /// Digest of the serialized document table.
    pub documents: String,
}

impl RegexIndexRuntime {
    /// Returns the validated base-layer content digests for parity checks.
    pub fn shared_scan_content_digests(&self) -> Option<RegexIndexContentDigests> {
        let loaded = self.loaded.as_ref()?;
        Some(RegexIndexContentDigests {
            lookup: loaded.base_files.lookup_digest.clone(),
            postings: loaded.base_files.postings_digest.clone(),
            documents: loaded.base_files.docs_digest.clone(),
        })
    }

    /// Returns base-layer document paths in document-id order.
    ///
    /// Duplicate lossy path keys are retained so parity tests can prove that
    /// non-UTF-8 names preserve the standalone regex scanner's behavior.
    pub fn shared_scan_document_paths(&self) -> Option<Vec<String>> {
        let loaded = self.loaded.as_ref()?;
        Some(
            loaded
                .base
                .docs
                .iter()
                .map(|document| document.path.clone())
                .collect(),
        )
    }
}

/// Returns whether a regex rebuild wants a path yielded by its repository walker.
pub fn wants_path(relative_path: &str) -> bool {
    is_normalized_repository_relative_path(relative_path)
        && !relative_path.starts_with(".git/")
        && !relative_path.starts_with(".packet28/")
        && !relative_path.starts_with("target/")
        && !relative_path.starts_with("node_modules/")
}

fn is_normalized_repository_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && path.split('/').all(|component| !component.is_empty())
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// Returns whether a discovered regex path needs its content read.
pub fn wants_content(metadata: &fs::Metadata) -> bool {
    metadata.len() <= MAX_INDEXED_FILE_BYTES as u64
}

/// A regex-specific full-rebuild session that borrows one content buffer at a time.
pub struct RegexIndexScanSession {
    root: PathBuf,
    include_tests: bool,
    discovered: BTreeSet<String>,
    docs: Vec<IndexedDocument>,
    writer: GenerationWriterLock,
    previous: Option<RegexIndexManifest>,
    previous_current_bytes: Option<Vec<u8>>,
    previous_previous_bytes: Option<Vec<u8>>,
    generation: u64,
    started_at_unix: u64,
}

impl RegexIndexScanSession {
    /// Starts a locked regex rebuild from a coordinator-owned discovery plan.
    ///
    /// # Errors
    ///
    /// Returns a typed filesystem or manifest error when the writer lock or
    /// pre-publication state cannot be read.
    pub fn begin(root: &Path, include_tests: bool, discovered_paths: &[String]) -> Result<Self> {
        if let Some(path) = discovered_paths
            .iter()
            .find(|path| !is_normalized_repository_relative_path(path))
        {
            return Err(SearchError::InvalidChangedPath { path: path.clone() });
        }
        let writer = acquire_writer_lock(root)?;
        let previous_current_bytes = read_optional_file(&manifest_path(root))?;
        let previous_previous_bytes = read_optional_file(&previous_manifest_path(root))?;
        let previous = load_runtime(root)
            .ok()
            .filter(RegexIndexRuntime::is_loaded)
            .map(|runtime| durable_manifest(&runtime.manifest));
        let generation = load_manifest(root)
            .generation
            .max(previous.as_ref().map_or(0, |manifest| manifest.generation))
            .saturating_add(1);
        Ok(Self {
            root: root.to_path_buf(),
            include_tests,
            discovered: discovered_paths
                .iter()
                .filter(|path| wants_path(path))
                .cloned()
                .collect(),
            docs: Vec::new(),
            writer,
            previous,
            previous_current_bytes,
            previous_previous_bytes,
            generation,
            started_at_unix: now_unix(),
        })
    }

    /// Returns the regex-specific progress total for this discovery plan.
    pub fn total_files(&self) -> usize {
        self.discovered.len()
    }

    /// Borrows one successfully read regex candidate and derives its grams.
    ///
    /// Empty, NUL-containing, and oversized files retain the existing
    /// full-rebuild omission behavior.
    ///
    /// # Errors
    ///
    /// Returns [`SearchError::CorruptIndex`] when `relative_path` was not part
    /// of the immutable discovery plan.
    pub fn ingest(
        &mut self,
        relative_path: &str,
        metadata: &fs::Metadata,
        bytes: &[u8],
    ) -> Result<()> {
        if !self.discovered.contains(relative_path) {
            return Err(SearchError::corrupt(format!(
                "shared regex scan path '{relative_path}' was not in the discovery plan"
            )));
        }
        if !wants_content(metadata) || bytes.is_empty() || bytes.contains(&0) {
            return Ok(());
        }
        self.docs.push(IndexedDocument {
            doc_id: 0,
            path: relative_path.to_string(),
            size: metadata.len(),
            mtime_secs: mtime_secs(metadata),
            fingerprint: blake3::hash(bytes).to_hex().to_string(),
            grams: build_indexed_grams(bytes),
        });
        Ok(())
    }

    /// Builds and validates immutable artifacts without publishing the manifest.
    ///
    /// # Errors
    ///
    /// Returns typed I/O, encoding, or corruption failures from base-layer
    /// construction and generation-record validation.
    pub fn prepare(mut self) -> Result<PreparedRegexIndexRuntime> {
        self.docs.sort_by(|left, right| left.path.cmp(&right.path));
        for (index, document) in self.docs.iter_mut().enumerate() {
            document.doc_id = u32::try_from(index)?;
        }
        let overlay_state = OverlayState::default();
        let mut manifest = RegexIndexManifest {
            schema_version: REGEX_INDEX_SCHEMA_VERSION,
            weight_table_version: WEIGHT_TABLE_VERSION,
            generation: self.generation,
            include_tests: self.include_tests,
            status: "ready".to_string(),
            last_build_started_at_unix: Some(self.started_at_unix),
            overlay_state_digest: Some(overlay_state_digest(&overlay_state)?),
            ..RegexIndexManifest::default()
        };
        let mut base_files = LayerFiles::base(self.generation);
        let base_layer = build_layer(&self.root, &self.docs, &mut base_files)?;
        manifest.total_files = self.docs.len();
        manifest.indexed_files = self.docs.len();
        manifest.base_commit = current_git_commit(&self.root);
        manifest.last_build_completed_at_unix = Some(now_unix());
        let record = RegexGenerationRecord {
            schema_version: REGEX_INDEX_SCHEMA_VERSION,
            generation: self.generation,
            manifest: manifest.clone(),
            base: base_files.clone(),
            segments: Vec::new(),
            overlay_state: overlay_state.clone(),
        };
        validate_generation_record(&record)?;
        save_generation_record(&self.root, &record)?;
        let runtime = RegexIndexRuntime {
            manifest,
            loaded: Some(Arc::new(LoadedIndex {
                base: Arc::new(base_layer),
                base_files,
                overlays: Vec::new(),
                overlay_state,
            })),
        };
        Ok(PreparedRegexIndexRuntime {
            root: self.root,
            _writer: self.writer,
            previous: self.previous,
            previous_current_bytes: self.previous_current_bytes,
            previous_previous_bytes: self.previous_previous_bytes,
            record,
            runtime,
            published: false,
            committed: false,
        })
    }
}

/// A validated regex generation awaiting manifest publication.
///
/// Dropping a published but uncommitted handle performs a best-effort rollback
/// to the exact manifest bytes captured before the scan began.
pub struct PreparedRegexIndexRuntime {
    root: PathBuf,
    _writer: GenerationWriterLock,
    previous: Option<RegexIndexManifest>,
    previous_current_bytes: Option<Vec<u8>>,
    previous_previous_bytes: Option<Vec<u8>>,
    record: RegexGenerationRecord,
    runtime: RegexIndexRuntime,
    published: bool,
    committed: bool,
}

impl PreparedRegexIndexRuntime {
    /// Publishes this validated generation while retaining rollback ownership.
    ///
    /// # Errors
    ///
    /// Returns a typed manifest error or rejects a duplicate publication.
    pub fn publish(&mut self) -> Result<()> {
        self.publish_with(publish_manifest)
    }

    fn publish_with<F>(&mut self, publish: F) -> Result<()>
    where
        F: FnOnce(&Path, Option<&RegexIndexManifest>, &RegexIndexManifest) -> Result<()>,
    {
        if self.published {
            return Err(SearchError::corrupt(
                "prepared regex generation was already published",
            ));
        }
        self.published = true;
        match publish(&self.root, self.previous.as_ref(), &self.runtime.manifest) {
            Ok(()) => Ok(()),
            Err(publication) => match self.rollback() {
                Ok(()) => Err(publication),
                Err(rollback) => Err(SearchError::FailureProvenance {
                    build: Box::new(publication),
                    persistence: Box::new(
                        rollback.context("failed to restore pre-publication regex manifests"),
                    ),
                }),
            },
        }
    }

    /// Restores both regex manifest files to their pre-publication bytes.
    ///
    /// # Errors
    ///
    /// Returns a typed filesystem error when either manifest cannot be restored.
    pub fn rollback(&mut self) -> Result<()> {
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

    /// Returns metadata for the validated generation without publishing it.
    pub fn manifest(&self) -> &RegexIndexManifest {
        &self.runtime.manifest
    }

    /// Returns the validated base-layer digests before publication.
    pub fn content_digests(&self) -> Option<RegexIndexContentDigests> {
        self.runtime.shared_scan_content_digests()
    }

    /// Finalizes a successfully paired publication and releases its writer lock.
    ///
    /// # Errors
    ///
    /// Returns [`SearchError::CorruptIndex`] if [`Self::publish`] has not succeeded.
    pub fn commit(mut self) -> Result<RegexIndexRuntime> {
        if !self.published {
            return Err(SearchError::corrupt(
                "prepared regex generation must be published before commit",
            ));
        }
        let _ = prune_generation_artifacts(&self.root, &self.record, self.previous.as_ref());
        self.committed = true;
        Ok(self.runtime.clone())
    }
}

impl Drop for PreparedRegexIndexRuntime {
    fn drop(&mut self) {
        if self.published && !self.committed {
            let _ = self.rollback();
        }
    }
}

fn read_optional_file(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("failed to preserve regex manifest '{}'", path.display())),
    }
}

fn restore_optional_file(path: PathBuf, bytes: Option<&[u8]>) -> Result<()> {
    match bytes {
        Some(bytes) => write_atomic(path, bytes),
        None => match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| {
                format!("failed to remove rolled-back manifest '{}'", path.display())
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepare_shared(root: &Path, paths: &[String]) -> PreparedRegexIndexRuntime {
        let mut session = RegexIndexScanSession::begin(root, true, paths).unwrap();
        for relative_path in paths {
            let path = root.join(relative_path);
            let metadata = fs::metadata(&path).unwrap();
            if wants_content(&metadata) {
                let bytes = fs::read(path).unwrap();
                session.ingest(relative_path, &metadata, &bytes).unwrap();
            }
        }
        session.prepare().unwrap()
    }

    #[test]
    fn borrowed_scan_matches_standard_layer_bytes_and_rolls_back_until_commit() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(
            root.join("src/lib.rs"),
            b"pub fn packet_search_literal() {}\n",
        )
        .unwrap();
        fs::write(root.join("docs/readme.md"), b"packet search literal\n").unwrap();
        let paths = vec!["docs/readme.md".to_string(), "src/lib.rs".to_string()];

        let standard = rebuild_full_index(root, true).unwrap();
        let expected_digests = standard.shared_scan_content_digests().unwrap();
        let mut prepared = prepare_shared(root, &paths);

        assert_eq!(prepared.content_digests().unwrap(), expected_digests);
        assert_eq!(
            load_runtime(root).unwrap().manifest.generation,
            standard.manifest.generation
        );
        prepared.publish().unwrap();
        assert!(load_runtime(root).unwrap().manifest.generation > standard.manifest.generation);
        prepared.rollback().unwrap();
        assert_eq!(
            load_runtime(root).unwrap().manifest.generation,
            standard.manifest.generation
        );

        prepared.publish().unwrap();
        let shared = prepared.commit().unwrap();
        assert_eq!(
            shared.shared_scan_content_digests().unwrap(),
            expected_digests
        );
    }

    #[test]
    fn dropping_a_published_generation_restores_the_previous_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), b"pub struct Searchable;\n").unwrap();
        let paths = vec!["src/lib.rs".to_string()];
        let standard = rebuild_full_index(root, true).unwrap();

        let mut prepared = prepare_shared(root, &paths);
        prepared.publish().unwrap();
        drop(prepared);

        assert_eq!(
            load_runtime(root).unwrap().manifest.generation,
            standard.manifest.generation
        );
    }

    #[test]
    fn failed_publication_restores_both_manifest_files_exactly() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), b"pub struct Searchable;\n").unwrap();
        let paths = vec!["src/lib.rs".to_string()];
        rebuild_full_index(root, true).unwrap();
        rebuild_full_index(root, true).unwrap();
        let current_path = manifest_path(root);
        let previous_path = previous_manifest_path(root);
        let expected_current = fs::read(&current_path).unwrap();
        let expected_previous = fs::read(&previous_path).unwrap();
        let mut prepared = prepare_shared(root, &paths);

        let error = prepared
            .publish_with(|root, _, _| {
                fs::write(manifest_path(root), b"partial current").unwrap();
                fs::write(previous_manifest_path(root), b"partial previous").unwrap();
                Err(SearchError::corrupt("injected publication failure"))
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
            RegexIndexScanSession::begin(directory.path(), true, &["../outside.rs".to_string()])
                .err()
                .unwrap();

        assert!(matches!(error, SearchError::InvalidChangedPath { .. }));
        assert!(!wants_path("../outside.rs"));
        assert!(!wants_path(r"src\\outside.rs"));
    }
}
