use packet28_reducer_core::{SearchRequest, SearchResult};
use suite_packet_core::search;

#[test]
fn legacy_search_paths_are_the_shared_wire_types() {
    let shared = search::SearchRequest {
        query: "DaemonRequest".to_string(),
        requested_paths: vec!["crates".to_string()],
        ..search::SearchRequest::default()
    };
    let legacy_request: SearchRequest = shared;
    let legacy_result: SearchResult = search::SearchResult {
        query: legacy_request.query,
        requested_paths: legacy_request.requested_paths,
        ..search::SearchResult::default()
    };

    assert_eq!(legacy_result.query, "DaemonRequest");
    assert_eq!(legacy_result.requested_paths, ["crates"]);
}
