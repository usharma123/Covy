//! Serializable packet-search requests and results.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct SearchRequest {
    pub query: String,
    pub requested_paths: Vec<String>,
    pub fixed_string: bool,
    pub case_sensitive: Option<bool>,
    pub whole_word: bool,
    pub context_lines: Option<usize>,
    pub max_matches_per_file: Option<usize>,
    pub max_total_matches: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct SearchMatch {
    pub path: String,
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct SearchGroup {
    pub path: String,
    pub match_count: usize,
    pub displayed_match_count: usize,
    pub truncated: bool,
    pub matches: Vec<SearchMatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct SearchResult {
    pub query: String,
    pub requested_paths: Vec<String>,
    pub resolved_paths: Vec<String>,
    pub match_count: usize,
    pub returned_match_count: usize,
    pub truncated: bool,
    pub paths: Vec<String>,
    pub regions: Vec<String>,
    pub symbols: Vec<String>,
    pub groups: Vec<SearchGroup>,
    pub compact_preview: String,
    pub diagnostics: Vec<String>,
    pub engine: Option<SearchEngineStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct SearchEngineStats {
    pub engine: String,
    pub index_generation: Option<u64>,
    pub base_commit: Option<String>,
    pub plan_kind: Option<String>,
    pub planner_fallback: Option<String>,
    pub stale_reason: Option<String>,
    pub candidates_examined: usize,
    pub candidate_files: usize,
    pub verified_files: usize,
    pub index_lookups: usize,
    pub postings_bytes_read: u64,
    pub fallback_reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn search_request_json_shape_is_stable() {
        let request = SearchRequest {
            query: "DaemonRequest".to_string(),
            requested_paths: vec!["crates".to_string()],
            fixed_string: true,
            case_sensitive: Some(false),
            whole_word: true,
            context_lines: Some(2),
            max_matches_per_file: Some(4),
            max_total_matches: Some(20),
        };

        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({
                "query": "DaemonRequest",
                "requested_paths": ["crates"],
                "fixed_string": true,
                "case_sensitive": false,
                "whole_word": true,
                "context_lines": 2,
                "max_matches_per_file": 4,
                "max_total_matches": 20
            })
        );
    }
}
