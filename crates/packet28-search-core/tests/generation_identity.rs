use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use packet28_search_core::{
    clear_index, load_runtime, rebuild_full_index, update_overlay_index, RegexIndexManifest,
    SearchError,
};
use tempfile::tempdir;

fn write_source(root: &Path, contents: &str) {
    fs::create_dir_all(root.join("src")).expect("source directory");
    fs::write(root.join("src/lib.rs"), contents).expect("source fixture");
}

fn index_dir(root: &Path) -> PathBuf {
    root.join(".packet28/index/regex-v1")
}

fn high_water_path(root: &Path) -> PathBuf {
    root.join(".packet28/index/.regex-v1.generation-high-water")
}

fn manifest_path(root: &Path) -> PathBuf {
    index_dir(root).join("manifest.json")
}

fn previous_manifest_path(root: &Path) -> PathBuf {
    index_dir(root).join("manifest.previous.json")
}

fn generation_record_path(root: &Path, generation: u64) -> PathBuf {
    index_dir(root).join(format!("generation-{generation:020}.json"))
}

fn file_snapshot(directory: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(directory)
        .expect("index directory")
        .map(|entry| {
            let entry = entry.expect("index entry");
            (
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(entry.path()).expect("index artifact"),
            )
        })
        .collect()
}

#[test]
fn clear_preserves_generation_identity_and_rejects_a_retained_handle() {
    let directory = tempdir().expect("temporary repository");
    let root = directory.path();
    write_source(root, "pub struct BeforeClear;\n");
    let retained = rebuild_full_index(root, true).expect("first generation");
    let reserved = fs::read(high_water_path(root)).expect("high-water mark");

    clear_index(root).expect("clear index");
    assert_eq!(
        fs::read(high_water_path(root)).expect("retained high-water mark"),
        reserved
    );
    write_source(root, "pub struct AfterClear;\n");
    let rebuilt = rebuild_full_index(root, true).expect("post-clear generation");
    let error = update_overlay_index(root, Some(&retained), &["src/lib.rs".to_string()])
        .expect_err("retained handle must be fenced");

    assert!(rebuilt.manifest.generation > retained.manifest.generation);
    assert!(matches!(
        error,
        SearchError::ConcurrentWriter { expected, actual }
            if expected == retained.manifest.generation
                && actual == rebuilt.manifest.generation
    ));
}

#[test]
fn legacy_clear_without_a_high_water_fences_the_retained_generation() {
    let directory = tempdir().expect("temporary repository");
    let root = directory.path();
    write_source(root, "pub struct LegacyClear;\n");
    let retained = rebuild_full_index(root, true).expect("legacy generation");
    fs::remove_file(high_water_path(root)).expect("remove modern high-water mark");

    clear_index(root).expect("clear legacy index");
    assert_eq!(
        serde_json::from_slice::<u64>(
            &fs::read(high_water_path(root)).expect("reconstructed high-water mark")
        )
        .expect("high-water json"),
        retained.manifest.generation
    );
    let rebuilt = rebuild_full_index(root, true).expect("post-clear generation");
    let error = update_overlay_index(root, Some(&retained), &["src/lib.rs".to_string()])
        .expect_err("retained legacy handle must be fenced");

    assert!(rebuilt.manifest.generation > retained.manifest.generation);
    assert!(matches!(error, SearchError::ConcurrentWriter { .. }));
}

#[test]
fn corrupt_manifest_rebuild_never_reuses_a_retained_generation() {
    let directory = tempdir().expect("temporary repository");
    let root = directory.path();
    write_source(root, "pub struct BeforeCorruption;\n");
    let retained = rebuild_full_index(root, true).expect("first generation");
    fs::write(manifest_path(root), b"{").expect("corrupt current manifest");

    write_source(root, "pub struct AfterCorruption;\n");
    let rebuilt = rebuild_full_index(root, true).expect("replacement generation");
    let error = update_overlay_index(root, Some(&retained), &["src/lib.rs".to_string()])
        .expect_err("retained handle must be fenced");

    assert!(rebuilt.manifest.generation > retained.manifest.generation);
    assert!(matches!(error, SearchError::ConcurrentWriter { .. }));
    assert_eq!(
        load_runtime(root)
            .expect("published runtime")
            .manifest
            .generation,
        rebuilt.manifest.generation
    );
}

#[test]
fn missing_current_manifest_recovers_previous_for_an_authoritative_update() {
    let directory = tempdir().expect("temporary repository");
    let root = directory.path();
    write_source(root, "pub struct First;\n");
    let first = rebuild_full_index(root, true).expect("first generation");
    write_source(root, "pub struct Second;\n");
    let second = rebuild_full_index(root, true).expect("second generation");
    fs::remove_file(manifest_path(root)).expect("remove current manifest");

    let recovered = load_runtime(root).expect("recover previous generation");
    assert!(recovered.is_loaded());
    assert_eq!(recovered.manifest.generation, first.manifest.generation);
    write_source(root, "pub struct Updated;\n");
    let updated = update_overlay_index(root, Some(&recovered), &["src/lib.rs".to_string()])
        .expect("update recovered generation");

    assert!(updated.manifest.generation > second.manifest.generation);
}

#[test]
fn schema_zero_current_manifest_recovers_the_explicit_previous_generation() {
    let directory = tempdir().expect("temporary repository");
    let root = directory.path();
    write_source(root, "pub struct First;\n");
    let first = rebuild_full_index(root, true).expect("first generation");
    write_source(root, "pub struct Second;\n");
    rebuild_full_index(root, true).expect("second generation");
    assert!(previous_manifest_path(root).exists());
    fs::write(manifest_path(root), b"{}").expect("schema-zero current manifest");

    let recovered = load_runtime(root).expect("recover previous generation");

    assert!(recovered.is_loaded());
    assert_eq!(recovered.manifest.generation, first.manifest.generation);
}

#[test]
fn same_generation_record_substitution_is_rejected_by_private_identity() {
    let directory = tempdir().expect("temporary repository");
    let root = directory.path();
    write_source(root, "pub struct Original;\n");
    let retained = rebuild_full_index(root, true).expect("first generation");
    let record_path = generation_record_path(root, retained.manifest.generation);
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(&record_path).expect("generation record"))
            .expect("record json");
    record["manifest"]["include_tests"] = serde_json::Value::Bool(false);
    record["manifest"]
        .as_object_mut()
        .expect("record manifest")
        .remove("publication_fingerprint");
    fs::write(
        &record_path,
        serde_json::to_vec_pretty(&record).expect("replacement record"),
    )
    .expect("replace record");
    let mut manifest: RegexIndexManifest =
        serde_json::from_slice(&fs::read(manifest_path(root)).expect("manifest"))
            .expect("manifest json");
    manifest.include_tests = false;
    manifest.publication_fingerprint = None;
    fs::write(
        manifest_path(root),
        serde_json::to_vec_pretty(&manifest).expect("replacement manifest"),
    )
    .expect("replace manifest");
    let reserved_before = fs::read(high_water_path(root)).expect("high-water mark");

    let error = update_overlay_index(root, Some(&retained), &["src/lib.rs".to_string()])
        .expect_err("same-number replacement must be fenced");

    assert!(matches!(
        error,
        SearchError::ConcurrentWriter { expected, actual }
            if expected == retained.manifest.generation && actual == expected
    ));
    assert_eq!(
        fs::read(high_water_path(root)).expect("unchanged high-water mark"),
        reserved_before
    );
}

#[test]
fn manifest_fingerprint_rejects_an_offline_generation_record_substitution() {
    let directory = tempdir().expect("temporary repository");
    let root = directory.path();
    write_source(root, "pub struct BoundRecord;\n");
    let runtime = rebuild_full_index(root, true).expect("bound generation");
    let record_path = generation_record_path(root, runtime.manifest.generation);
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(&record_path).expect("generation record"))
            .expect("record json");
    record["manifest"]["include_tests"] = serde_json::Value::Bool(false);
    fs::write(
        record_path,
        serde_json::to_vec_pretty(&record).expect("replacement record"),
    )
    .expect("replace record");

    let rejected = load_runtime(root).expect("rejected runtime state");

    assert!(!rejected.is_loaded());
    assert!(rejected
        .manifest
        .last_error
        .as_deref()
        .is_some_and(|error| error.contains("publication fingerprint validation")));
}

#[test]
fn max_minus_one_reserves_max_once_then_exhausts_without_artifact_writes() {
    let directory = tempdir().expect("temporary repository");
    let root = directory.path();
    write_source(root, "pub struct LastGeneration;\n");
    fs::create_dir_all(root.join(".packet28/index")).expect("index parent");
    fs::write(
        high_water_path(root),
        serde_json::to_vec(&(u64::MAX - 1)).expect("high-water json"),
    )
    .expect("seed high-water mark");

    let last = rebuild_full_index(root, true).expect("last generation");
    assert_eq!(last.manifest.generation, u64::MAX);
    let before = file_snapshot(&index_dir(root));
    let error = rebuild_full_index(root, true).expect_err("generation exhaustion");

    assert!(error.to_string().contains("generation space is exhausted"));
    assert_eq!(file_snapshot(&index_dir(root)), before);
    assert_eq!(
        serde_json::from_slice::<u64>(&fs::read(high_water_path(root)).expect("high-water mark"))
            .expect("high-water json"),
        u64::MAX
    );

    clear_index(root).expect("clear exhausted index");
    let error = rebuild_full_index(root, true).expect_err("durable exhaustion");
    assert!(error.to_string().contains("generation space is exhausted"));
    assert!(!index_dir(root).exists());
}

#[test]
fn observed_max_is_persisted_before_exhaustion_and_survives_clear() {
    let directory = tempdir().expect("temporary repository");
    let root = directory.path();
    write_source(root, "pub struct Exhausted;\n");
    fs::create_dir_all(index_dir(root)).expect("index directory");
    let sentinel = generation_record_path(root, u64::MAX);
    fs::write(&sentinel, b"sentinel").expect("observed max artifact");

    let error = rebuild_full_index(root, true).expect_err("observed exhaustion");
    assert!(error.to_string().contains("generation space is exhausted"));
    assert_eq!(
        fs::read(&sentinel).expect("unchanged sentinel"),
        b"sentinel"
    );
    assert_eq!(
        serde_json::from_slice::<u64>(
            &fs::read(high_water_path(root)).expect("reconciled high-water mark")
        )
        .expect("high-water json"),
        u64::MAX
    );

    clear_index(root).expect("clear observed artifact");
    let error = rebuild_full_index(root, true).expect_err("durable exhaustion");
    assert!(error.to_string().contains("generation space is exhausted"));
    assert!(!index_dir(root).exists());
}

#[test]
fn corrupt_high_water_is_rejected_before_artifact_construction() {
    let directory = tempdir().expect("temporary repository");
    let root = directory.path();
    write_source(root, "pub struct CorruptHighWater;\n");
    fs::create_dir_all(root.join(".packet28/index")).expect("index parent");
    fs::write(high_water_path(root), b"{").expect("corrupt high-water mark");

    let error = rebuild_full_index(root, true).expect_err("corrupt high-water mark");

    assert!(error
        .to_string()
        .contains("failed to decode regex generation high-water mark"));
    assert!(!index_dir(root).exists());
}

#[cfg(feature = "shared-repository-scan")]
mod shared_scan {
    use super::*;
    use packet28_search_core::shared_scan::{PreparedRegexIndexRuntime, RegexIndexScanSession};

    fn prepare(root: &Path) -> PreparedRegexIndexRuntime {
        let relative = "src/lib.rs".to_string();
        let path = root.join(&relative);
        let metadata = fs::metadata(&path).expect("source metadata");
        let bytes = fs::read(&path).expect("source bytes");
        let mut session = RegexIndexScanSession::begin(root, true, std::slice::from_ref(&relative))
            .expect("shared session");
        session
            .ingest(&relative, &metadata, &bytes)
            .expect("shared ingest");
        session.prepare().expect("shared prepare")
    }

    #[test]
    fn prepare_then_drop_consumes_its_generation() {
        let directory = tempdir().expect("temporary repository");
        let root = directory.path();
        write_source(root, "pub struct SharedPrepare;\n");
        let base = rebuild_full_index(root, true).expect("base generation");
        let prepared = prepare(root);
        let orphan = prepared.manifest().generation;
        assert!(orphan > base.manifest.generation);
        drop(prepared);

        let rebuilt = rebuild_full_index(root, true).expect("post-drop generation");
        assert!(rebuilt.manifest.generation > orphan);
    }

    #[test]
    fn published_reader_is_fenced_after_shared_drop_rolls_back() {
        let directory = tempdir().expect("temporary repository");
        let root = directory.path();
        write_source(root, "pub struct SharedRollback;\n");
        let base = rebuild_full_index(root, true).expect("base generation");
        let mut prepared = prepare(root);
        prepared.publish().expect("shared publication");
        let observed = load_runtime(root).expect("observe shared generation");
        assert!(observed.manifest.generation > base.manifest.generation);
        drop(prepared);
        assert_eq!(
            load_runtime(root)
                .expect("rolled-back generation")
                .manifest
                .generation,
            base.manifest.generation
        );

        let rebuilt = rebuild_full_index(root, true).expect("post-rollback generation");
        let error = update_overlay_index(root, Some(&observed), &["src/lib.rs".to_string()])
            .expect_err("rolled-back reader must be fenced");

        assert!(rebuilt.manifest.generation > observed.manifest.generation);
        assert!(matches!(error, SearchError::ConcurrentWriter { .. }));
    }
}
