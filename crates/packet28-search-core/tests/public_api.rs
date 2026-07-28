use std::error::Error as _;
use std::fs;

use packet28_reducer_core::SearchRequest;
use packet28_search_core::{
    clear_index, indexed_search, rebuild_full_index, RegexIndexRuntime, Result, SearchError,
};
use tempfile::tempdir;

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
