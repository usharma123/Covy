use std::collections::BTreeSet;
use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::sync::{Arc, Barrier};

use packet28_reducer_core::{SearchEngineStats, SearchRequest};

use crate::generation::*;
use crate::layer::*;
use crate::model::*;
use crate::paths::*;
use crate::postings::*;
use crate::publication::*;
use crate::query::*;
use crate::SearchError;

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

fn current_generation_record(root: &Path) -> RegexGenerationRecord {
    let manifest = load_manifest_strict(root).expect("manifest");
    load_generation_record(root, manifest.generation).expect("generation record")
}

fn current_base_files(root: &Path) -> LayerFiles {
    current_generation_record(root).base
}

fn rewrite_generation_record_unbound(root: &Path, record: &mut RegexGenerationRecord) {
    record.manifest.publication_fingerprint = None;
    save_manifest(root, &record.manifest).expect("rewrite manifest");
    write_atomic(
        generation_record_path(root, record.generation),
        &serde_json::to_vec_pretty(record).expect("record"),
    )
    .expect("rewrite record");
}

fn refresh_current_base_digests(root: &Path) {
    let mut record = current_generation_record(root);
    populate_layer_digests(root, &mut record.base).expect("refresh digests");
    rewrite_generation_record_unbound(root, &mut record);
}

fn corrupt_first_lookup_range(root: &Path, offset: u64, len: u32) {
    let path = regex_index_dir(root).join(current_base_files(root).lookup);
    let mut lookup = fs::read(&path).unwrap();
    assert!(lookup.len() >= LOOKUP_ROW_BYTES);
    lookup[8..16].copy_from_slice(&offset.to_le_bytes());
    lookup[16..20].copy_from_slice(&len.to_le_bytes());
    fs::write(path, lookup).unwrap();
    refresh_current_base_digests(root);
}

fn copy_layer_as_legacy(root: &Path, source: &LayerFiles, destination: &LayerFiles) {
    let directory = regex_index_dir(root);
    for (source_name, destination_name) in [
        (&source.lookup, &destination.lookup),
        (&source.postings, &destination.postings),
        (&source.docs, &destination.docs),
    ] {
        fs::copy(
            directory.join(source_name),
            directory.join(destination_name),
        )
        .unwrap();
    }
}

fn build_legacy_tombstone_fixture(
    root: &Path,
    with_overlay_document: bool,
    with_state_digest: bool,
) -> OverlayState {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/stale.rs"), "pub struct StaleBase;\n").unwrap();
    fs::write(root.join("src/changed.rs"), "pub struct Original;\n").unwrap();
    let base = rebuild_full_index(root, true).unwrap();
    fs::remove_file(root.join("src/stale.rs")).unwrap();
    let mut changed_paths = vec![String::from("src/stale.rs")];
    if with_overlay_document {
        fs::write(root.join("src/changed.rs"), "pub struct Replacement;\n").unwrap();
        changed_paths.push(String::from("src/changed.rs"));
    }
    let updated = update_overlay_index(root, Some(&base), &changed_paths).unwrap();
    let loaded = updated.loaded.as_ref().expect("updated index");
    let overlay_state = loaded.overlay_state.clone();
    copy_layer_as_legacy(root, &loaded.base_files, &LayerFiles::legacy_base());
    if let Some(segment) = loaded.overlays.first() {
        copy_layer_as_legacy(root, &segment.files, &LayerFiles::legacy_overlay());
    } else {
        build_layer(root, &[], &mut LayerFiles::legacy_overlay()).unwrap();
    }
    let mut manifest = updated.manifest.clone();
    manifest.publication_fingerprint = None;
    if !with_state_digest {
        manifest.overlay_state_digest = None;
    }
    save_manifest(root, &manifest).unwrap();
    write_atomic(
        overlay_state_path(root),
        &serde_json::to_vec_pretty(&overlay_state).unwrap(),
    )
    .unwrap();
    fs::remove_file(generation_record_path(root, manifest.generation)).unwrap();
    if previous_manifest_path(root).exists() {
        fs::remove_file(previous_manifest_path(root)).unwrap();
    }
    fs::write(root.join("src/stale.rs"), "pub struct StaleBase;\n").unwrap();
    overlay_state
}

fn stale_base_is_returned(root: &Path, runtime: &RegexIndexRuntime) -> bool {
    indexed_search(
        root,
        runtime,
        &SearchRequest {
            query: "StaleBase".to_string(),
            fixed_string: true,
            ..SearchRequest::default()
        },
    )
    .is_ok_and(|result| result.paths.iter().any(|path| path == "src/stale.rs"))
}

fn assert_legacy_state_is_corrupt(root: &Path, expected_error: &str) {
    let runtime = load_runtime(root).unwrap();
    assert!(
        !runtime.is_loaded()
            && runtime.manifest.status == "corrupt"
            && runtime
                .manifest
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains(expected_error))
            && !stale_base_is_returned(root, &runtime),
        "manifest={:?}",
        runtime.manifest
    );
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
    let path = regex_index_dir(dir.path()).join("corrupt.segment");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
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
    let path = regex_index_dir(dir.path()).join("complete.segment");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
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
fn successive_overlay_updates_search_only_the_newest_owner() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub struct Alpha;\n").unwrap();
    fs::write(root.join("src/keep.rs"), "pub struct Keep;\n").unwrap();
    let base = rebuild_full_index(root, true).unwrap();
    fs::write(root.join("src/lib.rs"), "pub struct Beta;\n").unwrap();
    let beta = update_overlay_index(root, Some(&base), &[String::from("src/lib.rs")]).unwrap();
    fs::write(root.join("src/lib.rs"), "pub struct Gamma;\n").unwrap();
    let gamma = update_overlay_index(root, Some(&beta), &[String::from("src/lib.rs")]).unwrap();

    for query in ["Alpha", "Beta", "Gamma"] {
        assert_parity(
            root,
            &gamma,
            SearchRequest {
                query: query.to_string(),
                fixed_string: true,
                ..SearchRequest::default()
            },
        );
    }
    let loaded = gamma.loaded.as_ref().expect("loaded");
    assert_eq!(loaded.overlays.len(), 2);
    assert_eq!(
        loaded.overlay_state.owners.get("src/lib.rs"),
        Some(&gamma.manifest.generation)
    );
    assert!(base.shares_base_with(&gamma));
}

#[test]
fn owner_mapping_mutation_recovers_instead_of_loading_stale_content() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub struct Alpha;\n").unwrap();
    let base = rebuild_full_index(root, true).unwrap();
    fs::write(root.join("src/lib.rs"), "pub struct Beta;\n").unwrap();
    let beta = update_overlay_index(root, Some(&base), &[String::from("src/lib.rs")]).unwrap();
    fs::write(root.join("src/lib.rs"), "pub struct Gamma;\n").unwrap();
    let gamma = update_overlay_index(root, Some(&beta), &[String::from("src/lib.rs")]).unwrap();
    let mut record = current_generation_record(root);
    assert_eq!(record.segments.len(), 2);
    record
        .overlay_state
        .owners
        .insert("src/lib.rs".to_string(), beta.manifest.generation);
    rewrite_generation_record_unbound(root, &mut record);

    let recovered = load_runtime(root).unwrap();

    assert!(recovered.is_loaded());
    assert_eq!(recovered.manifest.generation, beta.manifest.generation);
    assert_ne!(recovered.manifest.generation, gamma.manifest.generation);
    assert!(recovered
        .manifest
        .last_error
        .as_deref()
        .is_some_and(|error| error.contains("overlay state failed digest validation")));
}

#[test]
fn legacy_digestless_record_still_rejects_an_older_valid_owner() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub struct Alpha;\n").unwrap();
    let base = rebuild_full_index(root, true).unwrap();
    fs::write(root.join("src/lib.rs"), "pub struct Beta;\n").unwrap();
    let beta = update_overlay_index(root, Some(&base), &[String::from("src/lib.rs")]).unwrap();
    fs::write(root.join("src/lib.rs"), "pub struct Gamma;\n").unwrap();
    let _gamma = update_overlay_index(root, Some(&beta), &[String::from("src/lib.rs")]).unwrap();
    let mut manifest = load_manifest_strict(root).unwrap();
    manifest.overlay_state_digest = None;
    manifest.publication_fingerprint = None;
    save_manifest(root, &manifest).unwrap();
    let mut record = current_generation_record(root);
    record.manifest.overlay_state_digest = None;
    record
        .overlay_state
        .owners
        .insert("src/lib.rs".to_string(), beta.manifest.generation);
    rewrite_generation_record_unbound(root, &mut record);

    let recovered = load_runtime(root).unwrap();

    assert!(recovered.is_loaded());
    assert_eq!(recovered.manifest.generation, beta.manifest.generation);
    assert!(recovered
        .manifest
        .last_error
        .as_deref()
        .is_some_and(|error| error.contains("is not the newest document")));
}

#[test]
fn overlay_tombstone_delete_matches_reducer_before_and_after_reload() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub struct Alpha;\n").unwrap();
    fs::write(root.join("src/keep.rs"), "pub struct Keep;\n").unwrap();
    let base = rebuild_full_index(root, true).unwrap();
    fs::remove_file(root.join("src/lib.rs")).unwrap();

    let deleted = update_overlay_index(root, Some(&base), &[String::from("src/lib.rs")]).unwrap();
    let reloaded = load_runtime(root).unwrap();
    let request = SearchRequest {
        query: "Alpha".to_string(),
        fixed_string: true,
        ..SearchRequest::default()
    };

    assert_parity(root, &deleted, request.clone());
    assert_parity(root, &reloaded, request);
    assert!(deleted.loaded.as_ref().is_some_and(|loaded| loaded
        .overlay_state
        .deleted_paths
        .contains("src/lib.rs")
        && !loaded.overlay_state.owners.contains_key("src/lib.rs")));
}

#[test]
fn valid_digestless_legacy_overlay_state_preserves_tombstones() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let expected_state = build_legacy_tombstone_fixture(root, true, false);
    let runtime = load_runtime(root).unwrap();
    assert!(
        runtime.is_loaded()
            && runtime
                .loaded
                .as_ref()
                .is_some_and(|loaded| loaded.overlay_state == expected_state)
            && !stale_base_is_returned(root, &runtime),
        "manifest={:?}",
        runtime.manifest
    );
}

#[test]
fn missing_legacy_overlay_state_is_corrupt_without_resurrecting_base_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    build_legacy_tombstone_fixture(root, true, false);
    fs::remove_file(overlay_state_path(root)).unwrap();

    assert_legacy_state_is_corrupt(root, "failed to read legacy regex overlay state");
}

#[test]
fn malformed_legacy_overlay_state_is_corrupt_without_resurrecting_base_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    build_legacy_tombstone_fixture(root, true, false);
    fs::write(overlay_state_path(root), b"{").unwrap();

    assert_legacy_state_is_corrupt(root, "failed to decode legacy regex overlay state");
}

#[test]
fn unreadable_legacy_overlay_state_is_corrupt_without_resurrecting_base_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    build_legacy_tombstone_fixture(root, true, false);
    fs::remove_file(overlay_state_path(root)).unwrap();
    fs::create_dir(overlay_state_path(root)).unwrap();

    assert_legacy_state_is_corrupt(root, "failed to read legacy regex overlay state");
}

#[test]
fn empty_legacy_overlay_with_zero_file_count_still_loads_tombstones() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let expected_state = build_legacy_tombstone_fixture(root, false, false);
    let manifest = load_manifest_strict(root).unwrap();
    let runtime = load_runtime(root).unwrap();
    assert!(
        manifest.overlay_files == 0
            && manifest.overlay_segments == 0
            && runtime.is_loaded()
            && runtime
                .loaded
                .as_ref()
                .is_some_and(|loaded| loaded.overlay_state == expected_state)
            && !stale_base_is_returned(root, &runtime),
        "manifest={:?}",
        runtime.manifest
    );
}

#[test]
fn legacy_overlay_state_digest_rejects_a_well_formed_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    build_legacy_tombstone_fixture(root, true, true);
    fs::write(
        overlay_state_path(root),
        serde_json::to_vec_pretty(&OverlayState::default()).unwrap(),
    )
    .unwrap();

    assert_legacy_state_is_corrupt(root, "legacy regex overlay state failed digest validation");
}

#[test]
fn overlay_threshold_compaction_preserves_search_parity_and_base_arc() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn revision_0() {}\n").unwrap();
    let base = rebuild_full_index(root, true).unwrap();
    let mut runtime = base.clone();

    for revision in 1..=OVERLAY_COMPACTION_SEGMENTS {
        fs::write(
            root.join("src/lib.rs"),
            format!("pub fn revision_{revision}() {{}}\n"),
        )
        .unwrap();
        runtime =
            update_overlay_index(root, Some(&runtime), &[String::from("src/lib.rs")]).unwrap();
    }
    let reloaded = load_runtime(root).unwrap();
    let request = SearchRequest {
        query: format!("revision_{OVERLAY_COMPACTION_SEGMENTS}"),
        fixed_string: true,
        ..SearchRequest::default()
    };

    assert_eq!(runtime.manifest.overlay_segments, 1);
    assert!(base.shares_base_with(&runtime));
    assert_parity(root, &runtime, request.clone());
    assert_parity(root, &reloaded, request);
    assert!(current_generation_record(root).segments[0]
        .files
        .docs
        .contains("-compacted."));
}

#[test]
fn corrupt_overlay_generation_record_recovers_the_previous_generation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let base = build_fixture_index(root);
    fs::write(root.join("src/lib.rs"), "pub struct Replacement;\n").unwrap();
    let updated = update_overlay_index(root, Some(&base), &[String::from("src/lib.rs")]).unwrap();
    fs::write(
        generation_record_path(root, updated.manifest.generation),
        b"{",
    )
    .unwrap();

    let recovered = load_runtime(root).unwrap();

    assert!(recovered.is_loaded());
    assert_eq!(recovered.manifest.generation, base.manifest.generation);
    assert!(recovered
        .manifest
        .stale_reason
        .as_deref()
        .is_some_and(|reason| reason.contains("recovered generation")));
}

#[test]
fn every_missing_overlay_artifact_recovers_the_previous_generation() {
    for artifact in 0..3 {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let base = build_fixture_index(root);
        fs::write(root.join("src/lib.rs"), "pub struct Replacement;\n").unwrap();
        let updated =
            update_overlay_index(root, Some(&base), &[String::from("src/lib.rs")]).unwrap();
        let files = current_generation_record(root).segments[0].files.clone();
        let file_name = [&files.docs, &files.lookup, &files.postings][artifact];
        fs::remove_file(regex_index_dir(root).join(file_name)).unwrap();

        let recovered = load_runtime(root).unwrap();

        assert!(
            recovered.is_loaded()
                && recovered.manifest.generation == base.manifest.generation
                && recovered
                    .manifest
                    .last_error
                    .as_deref()
                    .is_some_and(|error| error.contains(file_name)),
            "artifact={file_name}, updated={:?}, recovered={:?}",
            updated.manifest,
            recovered.manifest
        );
    }
}

#[test]
fn truncated_overlay_lookup_and_postings_recover_previous_generation() {
    for artifact in ["lookup", "postings"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let base = build_fixture_index(root);
        fs::write(root.join("src/lib.rs"), "pub struct Replacement;\n").unwrap();
        let _updated =
            update_overlay_index(root, Some(&base), &[String::from("src/lib.rs")]).unwrap();
        let files = current_generation_record(root).segments[0].files.clone();
        let file_name = if artifact == "lookup" {
            &files.lookup
        } else {
            &files.postings
        };
        let path = regex_index_dir(root).join(file_name);
        let bytes = fs::read(&path).unwrap();
        fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();

        let recovered = load_runtime(root).unwrap();

        assert!(
            recovered.is_loaded() && recovered.manifest.generation == base.manifest.generation,
            "artifact={artifact}, recovered={:?}",
            recovered.manifest
        );
    }
}

#[test]
fn invalid_overlay_owner_and_non_increasing_segments_are_recoverable_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let base = build_fixture_index(root);
    fs::write(root.join("src/lib.rs"), "pub struct Beta;\n").unwrap();
    let first = update_overlay_index(root, Some(&base), &[String::from("src/lib.rs")]).unwrap();
    fs::write(root.join("src/lib.rs"), "pub struct Gamma;\n").unwrap();
    let second = update_overlay_index(root, Some(&first), &[String::from("src/lib.rs")]).unwrap();
    let mut record = current_generation_record(root);
    record.segments[1].generation = record.segments[0].generation;
    record
        .overlay_state
        .owners
        .insert("src/lib.rs".to_string(), second.manifest.generation + 1);
    rewrite_generation_record_unbound(root, &mut record);

    let recovered = load_runtime(root).unwrap();

    assert!(recovered.is_loaded());
    assert_eq!(recovered.manifest.generation, first.manifest.generation);
    assert!(recovered
        .manifest
        .last_error
        .as_deref()
        .is_some_and(|error| error.contains("non-increasing")));
}

#[test]
fn corrupt_current_manifest_recovers_only_the_explicit_backup() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let base = build_fixture_index(root);
    fs::write(root.join("src/lib.rs"), "pub struct Replacement;\n").unwrap();
    let _updated = update_overlay_index(root, Some(&base), &[String::from("src/lib.rs")]).unwrap();
    fs::write(manifest_path(root), b"{").unwrap();

    let recovered = load_runtime(root).unwrap();

    assert!(recovered.is_loaded());
    assert_eq!(recovered.manifest.generation, base.manifest.generation);
    assert!(recovered.manifest.last_error.is_some());
}

#[test]
fn unpublished_orphan_generation_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let runtime = build_fixture_index(root);
    let orphan_generation = runtime.manifest.generation + 1;
    fs::write(generation_record_path(root, orphan_generation), b"{").unwrap();
    fs::write(
        regex_index_dir(root).join(LayerFiles::overlay(orphan_generation, false).docs),
        b"partial",
    )
    .unwrap();

    let loaded = load_runtime(root).unwrap();

    assert!(loaded.is_loaded());
    assert_eq!(loaded.manifest.generation, runtime.manifest.generation);
}

#[test]
fn corrupt_new_base_recovers_the_previous_full_generation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let first = build_fixture_index(root);
    fs::write(root.join("src/new.rs"), "pub struct NewGeneration;\n").unwrap();
    let second = rebuild_full_index(root, true).unwrap();
    let files = current_base_files(root);
    fs::write(regex_index_dir(root).join(files.docs), b"corrupt").unwrap();

    let recovered = load_runtime(root).unwrap();

    assert!(recovered.is_loaded());
    assert_eq!(recovered.manifest.generation, first.manifest.generation);
    assert_ne!(recovered.manifest.generation, second.manifest.generation);
}

#[test]
fn changed_paths_cannot_escape_the_repository_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("workspace");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub struct Inside;\n").unwrap();
    fs::write(dir.path().join("outside.rs"), "pub struct Outside;\n").unwrap();
    let runtime = rebuild_full_index(&root, true).unwrap();

    let error =
        update_overlay_index(&root, Some(&runtime), &[String::from("../outside.rs")]).unwrap_err();

    assert!(matches!(error, SearchError::InvalidChangedPath { .. }));
    assert_eq!(
        load_runtime(&root).unwrap().manifest.generation,
        runtime.manifest.generation
    );
}

#[test]
fn concurrent_writers_return_an_explicit_generation_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub struct Alpha;\n").unwrap();
    let base = rebuild_full_index(root, true).unwrap();
    fs::write(root.join("src/charlie.rs"), "pub struct Charlie;\n").unwrap();
    fs::write(root.join("src/delta.rs"), "pub struct Delta;\n").unwrap();
    let barrier = Arc::new(Barrier::new(3));

    let results = std::thread::scope(|scope| {
        let handles = ["src/charlie.rs", "src/delta.rs"].map(|path| {
            let runtime = base.clone();
            let barrier = Arc::clone(&barrier);
            scope.spawn(move || {
                barrier.wait();
                update_overlay_index(root, Some(&runtime), &[path.to_string()])
            })
        });
        barrier.wait();
        handles.map(|handle| handle.join().unwrap())
    });

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(results.iter().any(|result| matches!(
        result,
        Err(SearchError::ConcurrentWriter { expected, actual })
            if *expected == base.manifest.generation
                && *actual == base.manifest.generation + 1
    )));
    let loaded = load_runtime(root).unwrap();
    assert!(loaded.is_loaded());
    assert_eq!(loaded.manifest.generation, base.manifest.generation + 1);
    let owners = &loaded.loaded.as_ref().unwrap().overlay_state.owners;
    assert_eq!(
        usize::from(owners.contains_key("src/charlie.rs"))
            + usize::from(owners.contains_key("src/delta.rs")),
        1
    );
}

#[test]
fn structurally_valid_docs_mutation_recovers_previous_generation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let base = build_fixture_index(root);
    fs::write(root.join("src/lib.rs"), "pub struct Replacement;\n").unwrap();
    let updated = update_overlay_index(root, Some(&base), &[String::from("src/lib.rs")]).unwrap();
    let files = current_generation_record(root).segments[0].files.clone();
    let docs_path = regex_index_dir(root).join(&files.docs);
    let raw = fs::read(&docs_path).unwrap();
    let mut docs = wincode::deserialize::<Vec<DocRecord>>(&raw).unwrap();
    docs[0].fingerprint.push('0');
    fs::write(&docs_path, wincode::serialize(&docs).unwrap()).unwrap();

    let recovered = load_runtime(root).unwrap();

    assert_eq!(recovered.manifest.generation, base.manifest.generation);
    assert_ne!(recovered.manifest.generation, updated.manifest.generation);
    assert!(recovered
        .manifest
        .last_error
        .as_deref()
        .is_some_and(|error| error.contains("digest validation")));
}

#[test]
fn retention_keeps_only_current_and_previous_full_generations() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub struct Alpha;\n").unwrap();
    let first = rebuild_full_index(root, true).unwrap();
    fs::write(root.join("src/second.rs"), "pub struct Second;\n").unwrap();
    let second = rebuild_full_index(root, true).unwrap();
    drop(first);
    fs::write(root.join("src/third.rs"), "pub struct Third;\n").unwrap();
    let third = rebuild_full_index(root, true).unwrap();
    let names = fs::read_dir(regex_index_dir(root))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
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
        6
    );
    assert!(!names.contains(&format!(
        "generation-{:020}.json",
        second.manifest.generation - 1
    )));
    assert!(names.contains(&format!(
        "generation-{:020}.json",
        second.manifest.generation
    )));
    assert!(names.contains(&format!(
        "generation-{:020}.json",
        third.manifest.generation
    )));
}

#[test]
fn corrupt_published_artifact_aborts_pruning_before_any_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let runtime = build_fixture_index(root);
    let files = current_base_files(root);
    fs::write(regex_index_dir(root).join(files.docs), b"corrupt").unwrap();
    let sentinel = regex_index_dir(root).join("base-00000000000000000000.lookup.dat");
    fs::write(&sentinel, b"must remain").unwrap();
    let writer = acquire_writer_lock(root).unwrap();

    let error = prune_generation_artifacts(root, &writer).unwrap_err();

    assert!(error.to_string().contains("failed to decode docs file"));
    assert_eq!(fs::read(sentinel).unwrap(), b"must remain");
    drop(runtime);
}

#[test]
fn immutable_artifact_writer_never_replaces_an_existing_target() {
    let dir = tempfile::tempdir().unwrap();
    let path = generation_record_path(dir.path(), 1);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"published").unwrap();

    let error = write_immutable(path.clone(), b"replacement").unwrap_err();

    assert!(error.to_string().contains("immutable index artifact"));
    assert_eq!(fs::read(path).unwrap(), b"published");
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
    let mut manifest = runtime.manifest;
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
    let files = current_base_files(root);
    fs::remove_file(regex_index_dir(root).join(&files.postings)).unwrap();
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
    let files = current_base_files(root);
    let lookup_path = regex_index_dir(root).join(&files.lookup);
    let original = fs::read(&lookup_path).unwrap();
    let complete_prefix_len = original.len() - LOOKUP_ROW_BYTES;

    for trailing in 1..LOOKUP_ROW_BYTES {
        fs::write(&lookup_path, &original[..complete_prefix_len + trailing]).unwrap();
        refresh_current_base_digests(root);
        let runtime = load_runtime(root).unwrap();
        let reason = runtime.manifest.stale_reason.as_deref().unwrap_or_default();

        assert!(
            !runtime.is_loaded()
                && runtime.manifest.status == "corrupt"
                && runtime.manifest.last_error.as_deref() == Some(reason)
                && reason.contains("failed to load base regex index layer")
                && reason.contains(&files.lookup)
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
    let files = current_base_files(root);
    let lookup = fs::read(regex_index_dir(root).join(&files.lookup)).unwrap();
    let postings_path = regex_index_dir(root).join(&files.postings);
    let postings = fs::read(&postings_path).unwrap();
    let final_row = &lookup[lookup.len() - LOOKUP_ROW_BYTES..];
    let offset = u64::from_le_bytes(final_row[8..16].try_into().unwrap()) as usize;
    let len = u32::from_le_bytes(final_row[16..20].try_into().unwrap()) as usize;
    assert_eq!(offset + len, postings.len());

    for prefix_len in 0..len {
        fs::write(&postings_path, &postings[..offset + prefix_len]).unwrap();
        refresh_current_base_digests(root);
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
    let files = current_base_files(root);
    for name in [&files.docs, &files.lookup, &files.postings] {
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
    let files = current_base_files(root);

    assert!(
        !runtime.is_loaded()
            && runtime.manifest.status == "corrupt"
            && runtime.manifest.stale_reason.as_deref() == Some(reason)
            && reason.contains("failed to load base regex index layer")
            && reason.contains(&files.lookup)
            && reason.contains(&files.postings)
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
    let postings_len = fs::metadata(regex_index_dir(root).join(current_base_files(root).postings))
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
fn incremental_publication_retains_the_validated_base_without_reloading_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let runtime = build_fixture_index(root);
    let original_record = current_generation_record(root);
    fs::write(root.join("src/lib.rs"), "pub struct Replacement;\n").unwrap();

    let updated =
        update_overlay_index(root, Some(&runtime), &[String::from("src/lib.rs")]).unwrap();
    let updated_record = current_generation_record(root);

    assert!(runtime.shares_base_with(&updated));
    assert_eq!(updated_record.base, original_record.base);
    assert_eq!(updated_record.segments.len(), 1);
    assert_eq!(updated.manifest.overlay_files, 1);
    assert_eq!(
        load_runtime(root).unwrap().manifest.generation,
        updated.manifest.generation
    );
}

#[test]
fn concurrent_readers_reject_stale_bytes_while_retaining_generation_owned_layers() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let runtime = build_fixture_index(root);
    fs::write(
        root.join("src/lib.rs"),
        "pub struct Alpha;\npub struct Replacement;\n",
    )
    .unwrap();
    let updated =
        update_overlay_index(root, Some(&runtime), &[String::from("src/lib.rs")]).unwrap();
    let old_request = SearchRequest {
        query: "Alpha".to_string(),
        fixed_string: true,
        ..SearchRequest::default()
    };
    let new_request = SearchRequest {
        query: "Replacement".to_string(),
        fixed_string: true,
        ..SearchRequest::default()
    };
    let barrier = Arc::new(Barrier::new(13));

    std::thread::scope(|scope| {
        let old_reader_handles = (0..4)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let request = old_request.clone();
                let runtime = &runtime;
                scope.spawn(move || {
                    barrier.wait();
                    indexed_search(root, runtime, &request).map(|result| result.match_count)
                })
            })
            .collect::<Vec<_>>();
        let new_reader_handles = (0..4)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let request = new_request.clone();
                let runtime = &updated;
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
                    load_runtime(root)
                        .map(|runtime| (runtime.is_loaded(), runtime.manifest.generation))
                })
            })
            .collect::<Vec<_>>();

        barrier.wait();
        for handle in old_reader_handles {
            assert!(matches!(
                handle.join().unwrap().unwrap_err(),
                SearchError::CandidateAuthentication { .. }
            ));
        }
        for handle in new_reader_handles {
            assert!(handle.join().unwrap().unwrap() > 0);
        }
        for handle in load_handles {
            assert_eq!(
                handle.join().unwrap().unwrap(),
                (true, updated.manifest.generation)
            );
        }
    });
    assert!(runtime.shares_base_with(&updated));
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
    let all_paths = loaded.all_indexed_paths(None);
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
