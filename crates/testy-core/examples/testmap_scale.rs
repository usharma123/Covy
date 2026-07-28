use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Instant;

use roaring::RoaringBitmap;
use serde_json::json;
use suite_packet_core::{
    DiffStatus, FileDiff, SparseFileCoverage, SparseTestCoverageRow, TestMapIndex,
};
use testy_core::impact::plan_impacted_tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Scale {
    tests: usize,
    files: usize,
    files_per_test: usize,
    lines_per_cell: usize,
    changed_files: usize,
    changed_lines: usize,
    max_tests: usize,
    iterations: usize,
}

impl Default for Scale {
    fn default() -> Self {
        Self {
            tests: 2_000,
            files: 1_000,
            files_per_test: 12,
            lines_per_cell: 16,
            changed_files: 40,
            changed_lines: 256,
            max_tests: 50,
            iterations: 8,
        }
    }
}

fn main() {
    let scale = parse_scale();
    let build_started = Instant::now();
    let (index, diffs, non_empty_cells) = sparse_fixture(scale);
    let build_micros = elapsed_micros(build_started);

    let serialize_started = Instant::now();
    let serialized = suite_foundation_core::cache::serialize_testmap(&index)
        .expect("serialize deterministic benchmark fixture");
    let serialize_micros = elapsed_micros(serialize_started);

    let deserialize_started = Instant::now();
    let restored = suite_foundation_core::cache::deserialize_testmap(&serialized)
        .expect("deserialize deterministic benchmark fixture");
    let deserialize_micros = elapsed_micros(deserialize_started);

    let mut plan_micros = Vec::with_capacity(scale.iterations);
    let mut plan_signature = String::new();
    let mut selected_tests = 0usize;
    for _ in 0..scale.iterations {
        let started = Instant::now();
        let plan = plan_impacted_tests(
            black_box(&restored),
            black_box(&diffs),
            scale.max_tests,
            0.9,
        );
        plan_micros.push(elapsed_micros(started));
        plan_signature = suite_packet_core::canonical_hash_json(&plan);
        selected_tests = plan.tests.len();
        black_box(plan);
    }
    plan_micros.sort_unstable();
    if scale == Scale::default() {
        assert_eq!(
            plan_signature, "dd7c2c02c56fa050d879ebd12b71e367982b96dac192187c2a0ed61028fe6171",
            "default fixture plan changed"
        );
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": "packet28-testmap-scale-v1",
            "implementation": "sparse_bitmap_v3",
            "profile": "release",
            "fixture": {
                "tests": scale.tests,
                "files": scale.files,
                "files_per_test": scale.files_per_test,
                "lines_per_cell": scale.lines_per_cell,
                "changed_files": scale.changed_files,
                "changed_lines_per_file": scale.changed_lines,
                "max_tests": scale.max_tests,
                "iterations": scale.iterations,
                "dense_cells": scale.tests.saturating_mul(scale.files),
                "non_empty_cells": non_empty_cells,
            },
            "build_micros": build_micros,
            "serialize_micros": serialize_micros,
            "deserialize_micros": deserialize_micros,
            "serialized_bytes": serialized.len(),
            "plan_micros": plan_micros,
            "plan_median_micros": median(&plan_micros),
            "selected_tests": selected_tests,
            "plan_signature": plan_signature,
        }))
        .expect("serialize benchmark result")
    );
}

fn parse_scale() -> Scale {
    let mut scale = Scale::default();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .unwrap_or_else(|| panic!("missing value for {flag}"));
        let parsed = value
            .parse::<usize>()
            .unwrap_or_else(|_| panic!("invalid integer for {flag}: {value}"));
        match flag.as_str() {
            "--tests" => scale.tests = parsed,
            "--files" => scale.files = parsed,
            "--files-per-test" => scale.files_per_test = parsed,
            "--lines-per-cell" => scale.lines_per_cell = parsed,
            "--changed-files" => scale.changed_files = parsed,
            "--changed-lines" => scale.changed_lines = parsed,
            "--max-tests" => scale.max_tests = parsed,
            "--iterations" => scale.iterations = parsed.max(1),
            _ => panic!("unknown option: {flag}"),
        }
    }
    assert!(scale.files > 0, "files must be non-zero");
    assert!(
        scale.files_per_test <= scale.files,
        "files-per-test must not exceed files"
    );
    scale
}

fn sparse_fixture(scale: Scale) -> (TestMapIndex, Vec<FileDiff>, usize) {
    let tests = (0..scale.tests)
        .map(|idx| format!("test_{idx:05}"))
        .collect::<Vec<_>>();
    let file_index = (0..scale.files)
        .map(|idx| format!("src/file_{idx:05}.rs"))
        .collect::<Vec<_>>();
    let mut sparse_coverage = Vec::with_capacity(scale.tests);
    let mut non_empty_cells = 0usize;
    for test_idx in 0..scale.tests {
        let mut row = BTreeMap::new();
        for cell_idx in 0..scale.files_per_test {
            let file_idx =
                (test_idx.saturating_mul(37) + cell_idx.saturating_mul(83)) % scale.files;
            let line_start = ((test_idx.saturating_mul(7) + file_idx.saturating_mul(11)) % 240) + 1;
            let lines = (0..scale.lines_per_cell)
                .map(|offset| (line_start + offset) as u32)
                .collect::<RoaringBitmap>();
            row.insert(file_idx, lines);
        }
        non_empty_cells += row.len();
        sparse_coverage.push(SparseTestCoverageRow {
            files: row
                .into_iter()
                .map(|(file_idx, lines)| SparseFileCoverage { file_idx, lines })
                .collect(),
        });
    }

    let index = TestMapIndex {
        tests,
        file_index: file_index.clone(),
        sparse_coverage,
        ..TestMapIndex::default()
    };
    let diffs = (0..scale.changed_files.min(scale.files))
        .map(|idx| {
            let file_idx = idx.saturating_mul(23) % scale.files;
            let changed_lines = (1..=scale.changed_lines.min(u32::MAX as usize))
                .map(|line| line as u32)
                .collect::<RoaringBitmap>();
            FileDiff {
                path: file_index[file_idx].clone(),
                old_path: None,
                status: DiffStatus::Modified,
                changed_lines,
            }
        })
        .collect();
    (index, diffs, non_empty_cells)
}

fn elapsed_micros(started: Instant) -> u128 {
    started.elapsed().as_micros()
}

fn median(values: &[u128]) -> u128 {
    values[values.len() / 2]
}
