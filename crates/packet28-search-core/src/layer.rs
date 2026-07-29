//! Repository scanning plus immutable layer construction and validation.

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap};
use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use ignore::WalkBuilder;
use memmap2::Mmap;

use crate::error::{Result, SearchError};
use crate::model::{
    DocRecord, HeapItem, IndexedGram, LayerFiles, LoadedLayer, LookupPostingMeta, PositionSummary,
    PostingEntry, PostingRow, LOOKUP_ROW_BYTES, MAX_INDEXED_FILE_BYTES, SEGMENT_DOC_BATCH_SIZE,
    SEGMENT_RECORD_BYTES, TEMP_FILE_NONCE,
};
use crate::paths::regex_index_dir;
use crate::postings::{
    build_indexed_grams, checked_posting_bounds, decode_postings, encode_postings, read_fixed_width,
};
use crate::support::{ensure_valid_index, mtime_secs, ResultContext};

pub(crate) fn scan_documents_with_progress<F>(
    root: &Path,
    mut on_progress: F,
) -> Result<Vec<IndexedDocument>>
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

pub(crate) fn discover_document_paths(root: &Path) -> Result<Vec<PathBuf>> {
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

pub(crate) struct IndexedDocument {
    pub(crate) doc_id: u32,
    pub(crate) path: String,
    pub(crate) size: u64,
    pub(crate) mtime_secs: u64,
    pub(crate) fingerprint: String,
    pub(crate) grams: Vec<IndexedGram>,
}

pub(crate) fn index_document(root: &Path, path: &Path) -> Result<Option<IndexedDocument>> {
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

pub(crate) fn build_layer(
    root: &Path,
    docs: &[IndexedDocument],
    files: &mut LayerFiles,
) -> Result<LoadedLayer> {
    fs::create_dir_all(regex_index_dir(root))?;
    let segment_files = write_segment_files(root, &files.lookup, docs)?;
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
    let docs_bytes = wincode::serialize(&serialized_docs)?;
    files.lookup_digest = artifact_digest(&lookup);
    files.postings_digest = artifact_digest(&postings);
    files.docs_digest = artifact_digest(&docs_bytes);
    write_atomic(regex_index_dir(root).join(&files.lookup), &lookup)?;
    write_atomic(regex_index_dir(root).join(&files.postings), &postings)?;
    write_atomic(regex_index_dir(root).join(&files.docs), &docs_bytes)?;
    load_layer(root, files)
}

pub(crate) fn write_segment_files(
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
pub(crate) struct SegmentFiles {
    pub(crate) paths: Vec<PathBuf>,
}

impl SegmentFiles {
    pub(crate) fn paths(&self) -> &[PathBuf] {
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

pub(crate) fn write_segment_file(path: &Path, pairs: &[(u64, u32, PositionSummary)]) -> Result<()> {
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

pub(crate) fn merge_and_cleanup_segment_files(
    segment_files: SegmentFiles,
) -> Result<(Vec<PostingRow>, Vec<u8>)> {
    merge_segment_files(segment_files.paths())
}

pub(crate) fn merge_segment_files(segment_paths: &[PathBuf]) -> Result<(Vec<PostingRow>, Vec<u8>)> {
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

pub(crate) fn read_segment_pair(
    reader: &mut impl Read,
) -> Result<Option<(u64, u32, PositionSummary)>> {
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
    let [hash_0, hash_1, hash_2, hash_3, hash_4, hash_5, hash_6, hash_7, doc_0, doc_1, doc_2, doc_3, position_0, position_1] =
        record;
    Ok(Some((
        u64::from_le_bytes([
            hash_0, hash_1, hash_2, hash_3, hash_4, hash_5, hash_6, hash_7,
        ]),
        u32::from_le_bytes([doc_0, doc_1, doc_2, doc_3]),
        PositionSummary::decode([position_0, position_1]),
    )))
}

pub(crate) fn flush_posting_group(
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

pub(crate) fn load_layer(root: &Path, files: &LayerFiles) -> Result<LoadedLayer> {
    let dir = regex_index_dir(root);
    validate_layer_file_names(files)?;
    let docs_path = dir.join(&files.docs);
    let lookup_path = dir.join(&files.lookup);
    let postings_path = dir.join(&files.postings);
    let docs_exists = docs_path.exists();
    let lookup_exists = lookup_path.exists();
    let postings_exists = postings_path.exists();
    let present_files = docs_exists as u8 + lookup_exists as u8 + postings_exists as u8;
    if present_files != 3 {
        let missing = [
            (!docs_exists).then_some(files.docs.as_str()),
            (!lookup_exists).then_some(files.lookup.as_str()),
            (!postings_exists).then_some(files.postings.as_str()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
        return Err(SearchError::corrupt(format!(
            "incomplete regex index layer '{}': expected docs, lookup, and postings files; found {present_files}/3; missing: {missing}",
            docs_path.display(),
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
    if files.has_digests() {
        verify_artifact_digest(&docs_path, &raw, &files.docs_digest)?;
        verify_artifact_digest(
            &lookup_path,
            lookup.as_deref().unwrap_or(&[]),
            &files.lookup_digest,
        )?;
        verify_artifact_digest(
            &postings_path,
            postings.as_deref().unwrap_or(&[]),
            &files.postings_digest,
        )?;
    }
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

pub(crate) fn write_atomic(path: PathBuf, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        SearchError::corrupt(format!("index artifact '{}' has no parent", path.display()))
    })?;
    fs::create_dir_all(parent)?;
    let nonce = TEMP_FILE_NONCE.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("artifact"),
        std::process::id(),
        nonce
    ));
    let result = (|| -> Result<()> {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.flush()?;
        drop(file);
        fs::rename(&tmp, &path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}
pub(crate) fn validate_layer_file_names(files: &LayerFiles) -> Result<()> {
    let mut unique = BTreeSet::new();
    for name in [&files.lookup, &files.postings, &files.docs] {
        let path = Path::new(name);
        ensure_valid_index!(
            !name.is_empty()
                && !path.is_absolute()
                && path.components().count() == 1
                && unique.insert(name),
            "regex generation references invalid or duplicate layer artifact name '{name}'"
        );
    }
    Ok(())
}

pub(crate) fn artifact_digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

pub(crate) fn verify_artifact_digest(path: &Path, bytes: &[u8], expected: &str) -> Result<()> {
    let actual = artifact_digest(bytes);
    ensure_valid_index!(
        actual == expected,
        "regex index artifact '{}' failed digest validation (expected {expected}, found {actual})",
        path.display()
    );
    Ok(())
}

pub(crate) fn populate_layer_digests(root: &Path, files: &mut LayerFiles) -> Result<()> {
    let directory = regex_index_dir(root);
    files.lookup_digest = artifact_digest(&fs::read(directory.join(&files.lookup))?);
    files.postings_digest = artifact_digest(&fs::read(directory.join(&files.postings))?);
    files.docs_digest = artifact_digest(&fs::read(directory.join(&files.docs))?);
    Ok(())
}

pub(crate) fn mmap_optional(path: &Path) -> Result<Option<Mmap>> {
    if !path.exists() || fs::metadata(path)?.len() == 0 {
        return Ok(None);
    }
    let file = File::open(path)?;
    // SAFETY: published generation files are immutable and are replaced only
    // by new generation-specific paths. `Mmap` owns the OS mapping after this
    // local file handle closes, and validation occurs before the layer is
    // exposed to readers.
    let map = unsafe { Mmap::map(&file)? };
    Ok(Some(map))
}

pub(crate) fn validate_layer_files(
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
        let hash = u64::from_le_bytes(read_fixed_width::<8>(row, 0, "lookup hash")?);
        let meta = LookupPostingMeta {
            offset: u64::from_le_bytes(read_fixed_width::<8>(row, 8, "lookup offset")?),
            len: u32::from_le_bytes(read_fixed_width::<4>(row, 16, "lookup length")?),
            doc_count: u32::from_le_bytes(read_fixed_width::<4>(row, 20, "lookup document count")?),
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
