use std::fs;
use std::path::Path;

use packet28_reducer_core::SearchRequest;
use packet28_search_core::{
    broker_internal_guarded_indexed_search_staged_batch, guarded_indexed_search_batch,
    rebuild_full_index, RegexIndexRuntime, SearchError,
};

fn build_fixture_index(root: &Path) -> RegexIndexRuntime {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        "pub struct Alpha;\npub fn alpha_service() {}\n",
    )
    .unwrap();
    rebuild_full_index(root, true).unwrap()
}

#[test]
fn guarded_batch_preserves_the_zero_candidate_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = build_fixture_index(dir.path());
    let request = SearchRequest {
        query: "term_absent_from_every_indexed_document".to_string(),
        fixed_string: true,
        ..SearchRequest::default()
    };

    let results = guarded_indexed_search_batch(dir.path(), &runtime, &[request]).unwrap();

    assert_eq!(results, [None]);
}

#[test]
fn guarded_batch_rejects_more_than_the_absolute_candidate_ceiling() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    for index in 0..65 {
        fs::write(
            root.join("src").join(format!("candidate_{index}.rs")),
            "const VALUE: &str = \"bounded_batch_common_term\";\n",
        )
        .unwrap();
    }
    let runtime = rebuild_full_index(root, true).unwrap();
    let request = SearchRequest {
        query: "bounded_batch_common_term".to_string(),
        fixed_string: true,
        ..SearchRequest::default()
    };

    let results = guarded_indexed_search_batch(root, &runtime, &[request]).unwrap();

    assert_eq!(results, [None]);
}

#[test]
fn staged_batch_applies_one_cumulative_candidate_read_ceiling_across_phases() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    for index in 0..40 {
        fs::write(
            root.join("src").join(format!("candidate_{index}.rs")),
            "const FIRST_BATCH_TERM: &str = \"SECOND_BATCH_TERM\";\n",
        )
        .unwrap();
    }
    let runtime = rebuild_full_index(root, true).unwrap();
    let request = |query: &str| SearchRequest {
        query: query.to_string(),
        fixed_string: true,
        ..SearchRequest::default()
    };

    let results = broker_internal_guarded_indexed_search_staged_batch(
        root,
        &runtime,
        &[request("FIRST_BATCH_TERM")],
        &[request("SECOND_BATCH_TERM")],
        |_| true,
    )
    .unwrap();

    assert_eq!(
        (
            results
                .primary
                .iter()
                .map(Option::is_some)
                .collect::<Vec<_>>(),
            results
                .deferred
                .unwrap()
                .iter()
                .map(Option::is_some)
                .collect::<Vec<_>>(),
        ),
        (vec![true], vec![false])
    );
}

#[test]
fn staged_batch_does_not_execute_an_unselected_deferred_request() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = build_fixture_index(dir.path());
    let primary = SearchRequest {
        query: "Alpha".to_string(),
        fixed_string: true,
        ..SearchRequest::default()
    };
    let invalid_deferred = SearchRequest {
        query: "(".to_string(),
        ..SearchRequest::default()
    };

    let results = broker_internal_guarded_indexed_search_staged_batch(
        dir.path(),
        &runtime,
        &[primary],
        &[invalid_deferred],
        |_| false,
    )
    .unwrap();

    assert_eq!((results.primary.len(), results.deferred), (1, None));
}

#[test]
fn empty_guarded_batch_requires_a_loaded_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = RegexIndexRuntime::default();

    let error = guarded_indexed_search_batch(dir.path(), &runtime, &[]).unwrap_err();

    assert!(matches!(error, SearchError::IndexNotLoaded));
}

#[test]
fn empty_guarded_batch_requires_a_ready_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = build_fixture_index(dir.path());
    runtime.manifest.status = "stale".to_string();

    let error = guarded_indexed_search_batch(dir.path(), &runtime, &[]).unwrap_err();

    assert!(matches!(error, SearchError::IndexNotReady { .. }));
}

#[test]
fn guarded_batch_rejects_candidate_bytes_that_differ_from_the_active_document() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    let source = root.join("src/lib.rs");
    fs::write(&source, "const TOKEN: &str = \"indexed\";\n").unwrap();
    let runtime = rebuild_full_index(root, true).unwrap();
    fs::write(&source, "const TOKEN: &str = \"altered\";\n").unwrap();
    let request = SearchRequest {
        query: "TOKEN".to_string(),
        fixed_string: true,
        ..SearchRequest::default()
    };

    let error = guarded_indexed_search_batch(root, &runtime, &[request]).unwrap_err();

    assert!(matches!(error, SearchError::CandidateAuthentication { .. }));
}

#[cfg(unix)]
#[test]
fn guarded_batch_rejects_a_symlink_even_when_its_bytes_match_the_index() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    let source = root.join("src/lib.rs");
    let bytes = "const TOKEN: &str = \"indexed\";\n";
    fs::write(&source, bytes).unwrap();
    let runtime = rebuild_full_index(root, true).unwrap();
    fs::write(outside.path().join("outside.rs"), bytes).unwrap();
    fs::remove_file(&source).unwrap();
    symlink(outside.path().join("outside.rs"), &source).unwrap();
    let request = SearchRequest {
        query: "TOKEN".to_string(),
        fixed_string: true,
        ..SearchRequest::default()
    };

    let error = guarded_indexed_search_batch(root, &runtime, &[request]).unwrap_err();

    assert!(matches!(error, SearchError::CandidateAuthentication { .. }));
}
