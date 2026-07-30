use std::error::Error as _;
use std::fs;
use std::path::Path;
#[cfg(unix)]
use std::process::Command;

use packet28_reducer_core::{SearchRequest, SearchResult};
use packet28_search_core::{
    clear_index, guarded_fallback_reason, indexed_search, load_runtime, rebuild_full_index,
    rebuild_full_index_with_progress, update_overlay_index, RegexIndexManifest, RegexIndexRuntime,
    Result, SearchError,
};
use tempfile::tempdir;

#[cfg(unix)]
fn run_fixture_git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("run fixture Git command");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.first().copied().unwrap_or("command"),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
fn initialize_clean_git_fixture(root: &Path) {
    fs::write(root.join(".gitignore"), ".packet28/\n").expect("Git ignore fixture");
    run_fixture_git(root, &["init", "--quiet"]);
    run_fixture_git(root, &["config", "user.name", "Packet28 Test"]);
    run_fixture_git(root, &["config", "user.email", "packet28@example.invalid"]);
    run_fixture_git(root, &["add", "."]);
    run_fixture_git(
        root,
        &["commit", "--quiet", "--no-gpg-sign", "-m", "fixture"],
    );
}

#[test]
fn public_result_exposes_the_typed_index_unavailable_variant() {
    let root = tempdir().expect("temporary repository");
    let runtime = RegexIndexRuntime::default();
    let request = SearchRequest {
        query: "packet".to_string(),
        ..SearchRequest::default()
    };

    let error = indexed_search(root.path(), &runtime, &request).unwrap_err();

    assert!(matches!(error, SearchError::IndexNotLoaded), "{error:?}");
}

#[test]
fn invalid_regex_preserves_the_parser_source() {
    let root = tempdir().expect("temporary repository");
    fs::create_dir_all(root.path().join("src")).expect("source directory");
    fs::write(root.path().join("src/lib.rs"), "pub fn packet() {}\n").expect("source fixture");
    let runtime = rebuild_full_index(root.path(), true).expect("index fixture");
    let request = SearchRequest {
        query: "(".to_string(),
        ..SearchRequest::default()
    };

    let error = indexed_search(root.path(), &runtime, &request).unwrap_err();
    let SearchError::InvalidRegexSyntax {
        source: typed_source,
        ..
    } = &error
    else {
        panic!("expected typed regex syntax failure, found {error:?}");
    };
    let chained_source = error.source().expect("regex parser source");

    assert_eq!(chained_source.to_string(), typed_source.to_string());
}

#[test]
fn contextual_filesystem_failure_keeps_the_io_source_chain() {
    let root = tempdir().expect("temporary repository");
    let index_parent = root.path().join(".packet28/index");
    fs::create_dir_all(&index_parent).expect("index parent");
    fs::write(index_parent.join("regex-v1"), b"not a directory").expect("blocking file");

    let error = clear_index(root.path()).unwrap_err();
    let SearchError::Context {
        source: typed_source,
        ..
    } = &error
    else {
        panic!("expected contextual I/O failure, found {error:?}");
    };
    let io_source = typed_source
        .source()
        .and_then(|source| source.downcast_ref::<std::io::Error>());

    assert!(
        matches!(typed_source.as_ref(), SearchError::Io { .. }) && io_source.is_some(),
        "error={error:?}"
    );
}

#[test]
fn public_result_alias_accepts_rebuild_results() {
    let root = tempdir().expect("temporary repository");
    let result: Result<RegexIndexRuntime> = rebuild_full_index(root.path(), true);

    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn public_error_is_send_sync_and_static() {
    fn assert_error_contract<T: std::error::Error + Send + Sync + 'static>() {}

    assert_error_contract::<SearchError>();
}

#[test]
fn root_entrypoint_signatures_remain_source_compatible() {
    type ProgressRebuild = fn(&Path, bool, fn(usize, usize)) -> Result<RegexIndexRuntime>;

    let _: fn(&Path) -> Result<RegexIndexRuntime> = load_runtime;
    let _: fn(&Path, bool) -> Result<RegexIndexRuntime> = rebuild_full_index;
    let _: ProgressRebuild = rebuild_full_index_with_progress::<fn(usize, usize)>;
    let _: fn(&Path, Option<&RegexIndexRuntime>, &[String]) -> Result<RegexIndexRuntime> =
        update_overlay_index;
    let _: fn(&Path) -> Result<()> = clear_index;
    let _: fn(&Path, &RegexIndexRuntime, &SearchRequest) -> Result<Option<String>> =
        guarded_fallback_reason;
    let _: fn(&Path, &RegexIndexRuntime, &SearchRequest) -> Result<SearchResult> = indexed_search;

    fn assert_runtime_contract<T: Clone + Default + Send + Sync + 'static>() {}
    assert_runtime_contract::<RegexIndexRuntime>();
}

#[test]
fn manifest_json_contract_round_trips_every_public_field() {
    let manifest = RegexIndexManifest {
        schema_version: 3,
        weight_table_version: 2,
        generation: 17,
        publication_fingerprint: Some("publication-digest".to_string()),
        include_tests: true,
        status: "ready".to_string(),
        total_files: 11,
        indexed_files: 8,
        overlay_files: 3,
        overlay_segments: 2,
        overlay_state_digest: Some("overlay-digest".to_string()),
        base_commit: Some("deadbeef".to_string()),
        workspace_clean_commit: Some("deadbeef".to_string()),
        stale_reason: Some("fixture-stale".to_string()),
        last_build_started_at_unix: Some(101),
        last_build_completed_at_unix: Some(102),
        last_error: Some("fixture-error".to_string()),
    };

    let value = serde_json::to_value(&manifest).expect("serialize public manifest");
    assert_eq!(
        value,
        serde_json::json!({
            "schema_version": 3,
            "weight_table_version": 2,
            "generation": 17,
            "publication_fingerprint": "publication-digest",
            "include_tests": true,
            "status": "ready",
            "total_files": 11,
            "indexed_files": 8,
            "overlay_files": 3,
            "overlay_segments": 2,
            "overlay_state_digest": "overlay-digest",
            "base_commit": "deadbeef",
            "workspace_clean_commit": "deadbeef",
            "stale_reason": "fixture-stale",
            "last_build_started_at_unix": 101,
            "last_build_completed_at_unix": 102,
            "last_error": "fixture-error"
        })
    );
    assert_eq!(
        serde_json::from_value::<RegexIndexManifest>(value).expect("deserialize public manifest"),
        manifest
    );
}

fn assert_public_search_parity(root: &Path, runtime: &RegexIndexRuntime, request: SearchRequest) {
    let indexed = indexed_search(root, runtime, &request).expect("indexed search");
    let reducer = packet28_reducer_core::search(root, &request).expect("reducer search");
    assert_eq!(indexed.match_count, reducer.match_count, "{request:?}");
    assert_eq!(indexed.paths, reducer.paths, "{request:?}");
    assert_eq!(indexed.regions, reducer.regions, "{request:?}");
}

#[test]
fn root_facade_preserves_lifecycle_and_search_parity() {
    let directory = tempdir().expect("temporary repository");
    let root = directory.path();
    fs::create_dir_all(root.join("src/nested")).expect("source directories");
    fs::write(
        root.join("src/lib.rs"),
        "pub fn Alpha_service() {}\npub fn beta_service() {}\n",
    )
    .expect("primary fixture");
    fs::write(root.join("src/nested/mod.rs"), "pub struct AlphaVariant;\n")
        .expect("nested fixture");

    let rebuilt = rebuild_full_index(root, true).expect("root rebuild");
    let loaded = load_runtime(root).expect("root load");
    assert!(loaded.is_loaded());
    assert!(rebuilt.shares_base_with(&rebuilt.clone()));

    for request in [
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
            query: "Alpha|beta".to_string(),
            ..SearchRequest::default()
        },
        SearchRequest {
            query: "Alpha_service".to_string(),
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
    ] {
        assert_public_search_parity(root, &loaded, request);
    }

    fs::write(
        root.join("src/lib.rs"),
        "pub fn Alpha_service() {}\npub fn Gamma_service() {}\n",
    )
    .expect("overlay fixture");
    let updated = update_overlay_index(root, Some(&loaded), &["src/lib.rs".to_string()])
        .expect("root overlay update");
    let reloaded = load_runtime(root).expect("root reload");
    assert_eq!(updated.manifest, reloaded.manifest);
    let gamma = indexed_search(
        root,
        &reloaded,
        &SearchRequest {
            query: "Gamma_service".to_string(),
            fixed_string: true,
            ..SearchRequest::default()
        },
    )
    .expect("search updated content");
    assert_eq!(gamma.match_count, 1);

    clear_index(root).expect("root clear");
    assert!(!load_runtime(root).expect("load cleared index").is_loaded());
}

#[cfg(feature = "shared-repository-scan")]
#[test]
fn shared_scan_public_surface_prepares_and_publishes_a_generation() {
    use packet28_search_core::shared_scan::{
        wants_content, wants_path, PreparedRegexIndexRuntime, RegexIndexContentDigests,
        RegexIndexScanSession, MAX_SHARED_SCAN_CONTENT_BYTES,
    };

    let _: fn(&str) -> bool = wants_path;
    let _: fn(&fs::Metadata) -> bool = wants_content;
    let _: fn(&Path, bool, &[String]) -> Result<RegexIndexScanSession> =
        RegexIndexScanSession::begin;
    let _: fn(RegexIndexScanSession) -> Result<PreparedRegexIndexRuntime> =
        RegexIndexScanSession::prepare;
    let _: Option<RegexIndexContentDigests> = None;
    assert_ne!(std::hint::black_box(MAX_SHARED_SCAN_CONTENT_BYTES), 0);

    let directory = tempdir().expect("temporary repository");
    let root = directory.path();
    fs::create_dir_all(root.join("src")).expect("source directory");
    let bytes = b"pub fn SharedScanLiteral() {}\n";
    let path = root.join("src/lib.rs");
    fs::write(&path, bytes).expect("shared scan fixture");
    let metadata = fs::metadata(&path).expect("shared scan metadata");
    let paths = vec!["src/lib.rs".to_string()];

    let mut session = RegexIndexScanSession::begin(root, true, &paths).expect("begin shared scan");
    assert_eq!(session.total_files(), 1);
    assert!(wants_path(&paths[0]) && wants_content(&metadata));
    session
        .ingest(&paths[0], &metadata, bytes)
        .expect("ingest borrowed bytes");
    let mut prepared = session.prepare().expect("prepare shared generation");
    assert_eq!(prepared.manifest().indexed_files, 1);
    let prepared_digests = prepared.content_digests().expect("prepared digests");
    prepared.publish().expect("publish shared generation");
    let runtime = prepared.commit().expect("commit shared generation");

    assert_eq!(
        runtime.shared_scan_content_digests(),
        Some(prepared_digests)
    );
    assert_eq!(
        runtime.shared_scan_document_paths(),
        Some(vec!["src/lib.rs".to_string()])
    );
    let result = indexed_search(
        root,
        &runtime,
        &SearchRequest {
            query: "SharedScanLiteral".to_string(),
            fixed_string: true,
            ..SearchRequest::default()
        },
    )
    .expect("search shared generation");
    assert_eq!(result.match_count, 1);
}

#[cfg(unix)]
#[test]
fn incremental_update_rejects_a_missing_path_beneath_a_symlink() {
    use std::os::unix::fs::symlink;

    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/lib.rs"), "pub fn indexed() {}\n").unwrap();
    symlink(outside.path(), root.path().join("alias")).unwrap();
    let runtime = rebuild_full_index(root.path(), true).unwrap();

    let error = update_overlay_index(
        root.path(),
        Some(&runtime),
        &["alias/missing.rs".to_string()],
    )
    .expect_err("missing path beneath a symlink was accepted");

    assert!(matches!(error, SearchError::InvalidChangedPath { .. }));
}

#[cfg(unix)]
#[test]
fn missing_requested_path_does_not_search_through_a_symlinked_directory() {
    use std::os::unix::fs::symlink;

    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/lib.rs"), "pub fn indexed() {}\n").unwrap();
    fs::write(outside.path().join("needle.rs"), "pub fn outside() {}\n").unwrap();
    symlink(outside.path(), root.path().join("alias")).unwrap();
    let runtime = rebuild_full_index(root.path(), true).unwrap();
    let request = SearchRequest {
        query: "indexed".to_string(),
        fixed_string: true,
        requested_paths: vec!["needle.rs".to_string()],
        ..SearchRequest::default()
    };

    let result = indexed_search(root.path(), &runtime, &request).unwrap();

    assert!(result.resolved_paths.is_empty());
}

#[cfg(unix)]
#[test]
fn full_rebuild_rejects_a_dirty_git_workspace_without_replacing_the_ready_generation() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn original() {}\n").unwrap();
    initialize_clean_git_fixture(root);
    let ready = rebuild_full_index(root, true).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn dirty() {}\n").unwrap();

    let error = rebuild_full_index(root, true)
        .expect_err("dirty workspace unexpectedly published a ready generation");

    assert!(matches!(error, SearchError::IndexNotReady { .. }));
    assert_eq!(
        load_runtime(root).unwrap().manifest.generation,
        ready.manifest.generation
    );
}

#[cfg(unix)]
#[test]
fn full_rebuild_rejects_clean_workspace_aba_bytes() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    let source = root.join("src/lib.rs");
    let original = "pub fn original_bytes() {}\n";
    let transient = "pub fn transient_bytes() {}\n";
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(&source, original).unwrap();
    initialize_clean_git_fixture(root);
    let ready = rebuild_full_index(root, true).unwrap();

    let error = rebuild_full_index_with_progress(root, true, |completed, total| {
        if completed == 0 {
            fs::write(&source, transient).unwrap();
        } else if completed == total {
            fs::write(&source, original).unwrap();
        }
    })
    .expect_err("clean-build ABA bytes unexpectedly published");

    assert!(matches!(error, SearchError::IndexNotReady { .. }));
    let retained = load_runtime(root).unwrap();
    assert!(retained.is_loaded());
    assert_eq!(retained.manifest.generation, ready.manifest.generation);
}

#[cfg(all(unix, feature = "shared-repository-scan"))]
#[test]
fn shared_rebuild_rejects_borrowed_bytes_restored_before_prepare() {
    use packet28_search_core::shared_scan::RegexIndexScanSession;

    let directory = tempdir().unwrap();
    let root = directory.path();
    let source = root.join("src/lib.rs");
    let original = b"pub fn original_shared_bytes() {}\n";
    let transient = b"pub fn transient_shared_bytes() {}\n";
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(&source, original).unwrap();
    initialize_clean_git_fixture(root);
    let paths = vec!["src/lib.rs".to_string()];
    let mut session = RegexIndexScanSession::begin(root, true, &paths).unwrap();
    fs::write(&source, transient).unwrap();
    let metadata = fs::metadata(&source).unwrap();
    session.ingest(&paths[0], &metadata, transient).unwrap();
    fs::write(&source, original).unwrap();

    let error = session
        .prepare()
        .err()
        .expect("restored borrowed bytes unexpectedly authenticated");

    assert!(matches!(error, SearchError::IndexNotReady { .. }));
}
