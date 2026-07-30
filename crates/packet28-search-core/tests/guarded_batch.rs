use std::fs;
use std::path::Path;
#[cfg(unix)]
use std::process::Command;

use packet28_reducer_core::SearchRequest;
use packet28_search_core::{
    broker_internal_guarded_indexed_search_batch, guarded_fallback_reason,
    guarded_indexed_search_batch, rebuild_full_index, BrokerInternalGuardedIndexedSearchSession,
    RegexIndexRuntime, SearchError,
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
fn broker_session_applies_one_cumulative_candidate_read_ceiling_across_batches() {
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

    let mut session = BrokerInternalGuardedIndexedSearchSession::new();
    let primary = broker_internal_guarded_indexed_search_batch(
        root,
        &runtime,
        &[request("FIRST_BATCH_TERM")],
        &mut session,
    )
    .unwrap();
    let deferred = broker_internal_guarded_indexed_search_batch(
        root,
        &runtime,
        &[request("SECOND_BATCH_TERM")],
        &mut session,
    )
    .unwrap();

    assert_eq!(
        (
            primary.iter().map(Option::is_some).collect::<Vec<_>>(),
            deferred.iter().map(Option::is_some).collect::<Vec<_>>(),
        ),
        (vec![true], vec![false])
    );
}

#[test]
fn explicit_invalid_scope_returns_zero_even_when_the_plan_is_broad() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = build_fixture_index(dir.path());
    let request = SearchRequest {
        query: ".*".to_string(),
        requested_paths: vec!["../outside.rs".to_string()],
        ..SearchRequest::default()
    };

    let fallback = guarded_fallback_reason(dir.path(), &runtime, &request).unwrap();
    let results = guarded_indexed_search_batch(dir.path(), &runtime, &[request]).unwrap();
    let result = results[0]
        .as_ref()
        .expect("an explicitly empty scope is an authoritative empty result");

    assert_eq!(fallback, None);
    assert_eq!(result.match_count, 0);
    assert!(result.paths.is_empty());
    assert!(result.resolved_paths.is_empty());
}

#[test]
fn invalid_regex_is_rejected_before_an_explicit_empty_scope() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = build_fixture_index(dir.path());
    let request = SearchRequest {
        query: "(".to_string(),
        requested_paths: vec!["../outside.rs".to_string()],
        ..SearchRequest::default()
    };

    let error = guarded_fallback_reason(dir.path(), &runtime, &request).unwrap_err();

    assert!(matches!(error, SearchError::InvalidRegexSyntax { .. }));
}

#[test]
fn guarded_batch_bounds_requested_scope_resolution_work() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = build_fixture_index(dir.path());
    let request = SearchRequest {
        query: "Alpha".to_string(),
        fixed_string: true,
        requested_paths: (0..17).map(|index| format!("missing-{index}.rs")).collect(),
        ..SearchRequest::default()
    };

    let error = guarded_indexed_search_batch(dir.path(), &runtime, &[request]).unwrap_err();

    assert!(
        matches!(
            error,
            SearchError::IndexNotReady { ref reason }
                if reason.contains("requested paths") && reason.contains("maximum is 16")
        ),
        "{error:?}"
    );
}

#[cfg(unix)]
#[test]
fn broker_batch_returns_no_results_when_final_freshness_attestation_fails() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub struct Alpha;\n").unwrap();
    fs::write(root.join("src/unrelated.rs"), "pub struct Unrelated;\n").unwrap();
    fs::write(root.join(".gitignore"), ".packet28/\n").unwrap();
    run_fixture_git(root, &["init", "--quiet"]);
    run_fixture_git(root, &["config", "user.name", "Packet28 Test"]);
    run_fixture_git(root, &["config", "user.email", "packet28@example.invalid"]);
    run_fixture_git(root, &["add", "."]);
    run_fixture_git(
        root,
        &["commit", "--quiet", "--no-gpg-sign", "-m", "fixture"],
    );
    let runtime = rebuild_full_index(root, true).unwrap();
    fs::write(
        root.join("src/unrelated.rs"),
        "pub struct DirtyUnrelated;\n",
    )
    .unwrap();
    let request = SearchRequest {
        query: "Alpha".to_string(),
        fixed_string: true,
        ..SearchRequest::default()
    };
    let mut session = BrokerInternalGuardedIndexedSearchSession::new();

    let error = broker_internal_guarded_indexed_search_batch(
        root,
        &runtime,
        &[request.clone()],
        &mut session,
    )
    .unwrap_err();

    assert!(matches!(error, SearchError::IndexNotReady { .. }));
    fs::write(root.join("src/unrelated.rs"), "pub struct Unrelated;\n").unwrap();
    let reuse_error =
        broker_internal_guarded_indexed_search_batch(root, &runtime, &[request], &mut session)
            .unwrap_err();
    assert!(
        matches!(
            reuse_error,
            SearchError::IndexNotReady { ref reason }
                if reason.contains("cannot be reused after a failed batch")
        ),
        "{reuse_error:?}"
    );
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
