use assert_cmd::Command;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub struct CorrelationPackets {
    pub diff: PathBuf,
    pub impact: PathBuf,
    pub stack: PathBuf,
    pub build: PathBuf,
    pub map: PathBuf,
}

pub fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

pub fn write_correlation_packets(dir: &Path) -> CorrelationPackets {
    let packets = CorrelationPackets {
        diff: dir.join("diff.json"),
        impact: dir.join("impact.json"),
        stack: dir.join("stack.json"),
        build: dir.join("build.json"),
        map: dir.join("map.json"),
    };

    write_packet_value(
        &packets.diff,
        &json!({
            "version": "1",
            "tool": "diffy",
            "kind": "diff_analyze",
            "hash": "diff-hash",
            "summary": "changed StopWatch",
            "files": [{"path": "src/StopWatch.java", "relevance": 1.0}],
            "symbols": [],
            "budget_cost": {"est_tokens": 1, "est_bytes": 1, "runtime_ms": 1, "tool_calls": 1},
            "provenance": {"inputs": ["diff"], "generated_at_unix": 1},
            "payload": {
                "gate_result": {"passed": true, "violations": []},
                "diffs": [{"path": "src/StopWatch.java", "old_path": null, "status": "Modified", "changed_lines": [10, 11]}]
            }
        }),
    );
    write_packet_value(
        &packets.impact,
        &json!({
            "version": "1",
            "tool": "testy",
            "kind": "test_impact",
            "hash": "impact-hash",
            "summary": "impact",
            "files": [],
            "symbols": [{"name": "StopWatchTest#testSplit", "kind": "test_id", "relevance": 1.0}],
            "budget_cost": {"est_tokens": 1, "est_bytes": 1, "runtime_ms": 1, "tool_calls": 1},
            "provenance": {"inputs": ["testmap.bin"], "generated_at_unix": 1},
            "payload": {
                "result": {
                    "selected_tests": ["StopWatchTest#testSplit"],
                    "smoke_tests": [],
                    "missing_mappings": [],
                    "confidence": 0.9,
                    "stale": false,
                    "escalate_full_suite": false
                },
                "known_tests": 1,
                "print_command": null
            }
        }),
    );
    write_packet_value(
        &packets.stack,
        &json!({
            "version": "1",
            "tool": "stacky",
            "kind": "stack_slice",
            "hash": "stack-hash",
            "summary": "stack",
            "files": [{"path": "src/ArrayUtils.java", "relevance": 1.0}],
            "symbols": [],
            "budget_cost": {"est_tokens": 1, "est_bytes": 1, "runtime_ms": 1, "tool_calls": 1},
            "provenance": {"inputs": ["stack.log"], "generated_at_unix": 1},
            "payload": {
                "schema_version": "stacky.slice.v1",
                "source": "stack.log",
                "total_failures": 1,
                "unique_failures": 1,
                "duplicates_removed": 0,
                "failures": []
            }
        }),
    );
    write_packet_value(
        &packets.build,
        &json!({
            "version": "1",
            "tool": "buildy",
            "kind": "build_reduce",
            "hash": "build-hash",
            "summary": "build",
            "files": [{"path": "src/CharUtils.java", "relevance": 1.0}],
            "symbols": [],
            "budget_cost": {"est_tokens": 1, "est_bytes": 1, "runtime_ms": 1, "tool_calls": 1},
            "provenance": {"inputs": ["build.log"], "generated_at_unix": 1},
            "payload": {
                "schema_version": "buildy.reduce.v1",
                "source": "build.log",
                "total_diagnostics": 1,
                "unique_diagnostics": 1,
                "duplicates_removed": 0,
                "groups": [],
                "ordered_fixes": []
            }
        }),
    );
    write_packet_value(
        &packets.map,
        &json!({
            "version": "1",
            "tool": "mapy",
            "kind": "repo_map",
            "hash": "map-hash",
            "summary": "map",
            "files": [
                {"path": "src/StopWatch.java", "relevance": 1.0},
                {"path": "src/ArrayUtils.java", "relevance": 0.8}
            ],
            "symbols": [],
            "budget_cost": {"est_tokens": 1, "est_bytes": 1, "runtime_ms": 1, "tool_calls": 1},
            "provenance": {"inputs": ["repo"], "generated_at_unix": 1},
            "payload": {
                "files_ranked": [{"file_idx": 0, "score": 1.0}, {"file_idx": 1, "score": 0.8}],
                "symbols_ranked": [],
                "edges": [],
                "focus_hits": [],
                "truncation": {"files_dropped": 0, "symbols_dropped": 0, "edges_dropped": 0}
            }
        }),
    );

    packets
}

fn write_packet_value(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
}
