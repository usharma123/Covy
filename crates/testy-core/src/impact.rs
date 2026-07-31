use std::collections::{BTreeMap, BTreeSet};

use roaring::RoaringBitmap;
use suite_packet_core::gate::{ImpactPlan, ImpactResult, PlannedTest, UncoveredBlock};
use suite_packet_core::testmap::SparseTestCoverageRow;

use crate::model::FileDiff;
use crate::testmap::TestMapIndex;

pub fn select_impacted_tests(index: &TestMapIndex, diffs: &[FileDiff]) -> ImpactResult {
    let mut tests: BTreeSet<String> = BTreeSet::new();
    let mut missing = BTreeSet::new();

    for diff in diffs {
        let mut matched = false;
        if let Some(candidates) = index.file_to_tests.get(&diff.path) {
            for t in candidates {
                tests.insert(t.clone());
            }
            matched = true;
        }

        if let Some(old_path) = diff.old_path.as_deref() {
            if let Some(candidates) = index.file_to_tests.get(old_path) {
                for t in candidates {
                    tests.insert(t.clone());
                }
                matched = true;
            }
        }

        if !matched {
            missing.insert(diff.path.clone());
        }
    }

    ImpactResult {
        selected_tests: tests.into_iter().collect(),
        smoke_tests: Vec::new(),
        missing_mappings: missing.into_iter().collect(),
        stale: false,
        confidence: 1.0,
        escalate_full_suite: false,
    }
}

pub fn plan_impacted_tests(
    index: &TestMapIndex,
    diffs: &[FileDiff],
    max_tests: usize,
    target_coverage: f64,
) -> ImpactPlan {
    let mut plan = ImpactPlan {
        next_command: "covy impact run --plan plan.json -- <your-test-command-template>"
            .to_string(),
        ..Default::default()
    };

    let file_to_idx: BTreeMap<&str, usize> = index
        .file_index
        .iter()
        .enumerate()
        .map(|(i, f)| (f.as_str(), i))
        .collect();

    let mut mapped_remaining: BTreeMap<usize, RoaringBitmap> = BTreeMap::new();
    let mut mapped_file_names: BTreeMap<usize, String> = BTreeMap::new();
    let mut unmapped_remaining: BTreeMap<String, RoaringBitmap> = BTreeMap::new();

    for diff in diffs {
        if diff.changed_lines.is_empty() {
            continue;
        }

        let mapped_idx = file_to_idx.get(diff.path.as_str()).copied().or_else(|| {
            diff.old_path
                .as_deref()
                .and_then(|p| file_to_idx.get(p).copied())
        });

        if let Some(file_idx) = mapped_idx {
            mapped_file_names
                .entry(file_idx)
                .or_insert_with(|| index.file_index[file_idx].clone());
            mapped_remaining
                .entry(file_idx)
                .or_default()
                .extend(diff.changed_lines.iter());
        } else {
            unmapped_remaining
                .entry(diff.path.clone())
                .or_default()
                .extend(diff.changed_lines.iter());
        }
    }

    plan.changed_lines_total =
        total_bitmap_lines(&mapped_remaining) + total_bitmap_lines_by_name(&unmapped_remaining);
    if plan.changed_lines_total == 0 {
        plan.plan_coverage_pct = 1.0;
        return plan;
    }

    let test_rows = index.sparse_coverage_rows();
    let max_index_tests = index.tests.len().min(test_rows.len());
    let original_overlaps = test_rows
        .iter()
        .take(max_index_tests)
        .map(|row| test_gain_against_remaining(row, &mapped_remaining))
        .collect::<Vec<_>>();
    let mut selected = vec![false; max_index_tests];
    let mut selected_count = 0usize;

    while plan.tests.len() < max_tests && selected_count < max_index_tests {
        let mut best: Option<(usize, u64, u64)> = None; // idx, gain, overlap

        for test_idx in 0..max_index_tests {
            if selected[test_idx] {
                continue;
            }
            let gain = test_gain_against_remaining(&test_rows[test_idx], &mapped_remaining);
            if gain == 0 {
                continue;
            }
            let overlap = original_overlaps[test_idx];

            best = match best {
                None => Some((test_idx, gain, overlap)),
                Some((best_idx, best_gain, best_overlap)) => {
                    if gain > best_gain
                        || (gain == best_gain && overlap > best_overlap)
                        || (gain == best_gain
                            && overlap == best_overlap
                            && index.tests[test_idx] < index.tests[best_idx])
                    {
                        Some((test_idx, gain, overlap))
                    } else {
                        Some((best_idx, best_gain, best_overlap))
                    }
                }
            };
        }

        let Some((winner_idx, winner_gain, winner_overlap)) = best else {
            break;
        };

        selected[winner_idx] = true;
        selected_count += 1;
        subtract_test_from_remaining(&test_rows[winner_idx], &mut mapped_remaining);

        let winner_id = index.tests[winner_idx].clone();
        plan.tests.push(PlannedTest {
            id: winner_id.clone(),
            name: winner_id,
            estimated_overlap_lines: winner_overlap,
            marginal_gain_lines: winner_gain,
        });

        let remaining_total =
            total_bitmap_lines(&mapped_remaining) + total_bitmap_lines_by_name(&unmapped_remaining);
        plan.changed_lines_covered_by_plan =
            plan.changed_lines_total.saturating_sub(remaining_total);
        plan.plan_coverage_pct =
            plan.changed_lines_covered_by_plan as f64 / plan.changed_lines_total as f64;

        if plan.plan_coverage_pct >= target_coverage {
            break;
        }
    }

    if plan.changed_lines_total > 0 {
        let remaining_total =
            total_bitmap_lines(&mapped_remaining) + total_bitmap_lines_by_name(&unmapped_remaining);
        plan.changed_lines_covered_by_plan =
            plan.changed_lines_total.saturating_sub(remaining_total);
        plan.plan_coverage_pct =
            plan.changed_lines_covered_by_plan as f64 / plan.changed_lines_total as f64;
    }

    plan.uncovered_blocks =
        build_uncovered_blocks(&mapped_remaining, &mapped_file_names, &unmapped_remaining);
    plan
}

fn total_bitmap_lines(map: &BTreeMap<usize, RoaringBitmap>) -> u64 {
    map.values().map(|b| b.len()).sum()
}

fn total_bitmap_lines_by_name(map: &BTreeMap<String, RoaringBitmap>) -> u64 {
    map.values().map(|b| b.len()).sum()
}

fn test_gain_against_remaining(
    test_row: &SparseTestCoverageRow,
    mapped_remaining: &BTreeMap<usize, RoaringBitmap>,
) -> u64 {
    test_row
        .files
        .iter()
        .filter_map(|cell| {
            mapped_remaining
                .get(&cell.file_idx)
                .map(|remaining| remaining.intersection_len(&cell.lines))
        })
        .sum()
}

fn subtract_test_from_remaining(
    test_row: &SparseTestCoverageRow,
    mapped_remaining: &mut BTreeMap<usize, RoaringBitmap>,
) {
    for cell in &test_row.files {
        if let Some(remaining) = mapped_remaining.get_mut(&cell.file_idx) {
            *remaining -= &cell.lines;
        }
    }
    mapped_remaining.retain(|_, remaining| !remaining.is_empty());
}

fn build_uncovered_blocks(
    mapped_remaining: &BTreeMap<usize, RoaringBitmap>,
    mapped_file_names: &BTreeMap<usize, String>,
    unmapped_remaining: &BTreeMap<String, RoaringBitmap>,
) -> Vec<UncoveredBlock> {
    let mut by_file: BTreeMap<&str, &RoaringBitmap> = BTreeMap::new();

    for (file_idx, lines) in mapped_remaining {
        if let Some(name) = mapped_file_names.get(file_idx) {
            by_file.insert(name, lines);
        }
    }
    for (file, lines) in unmapped_remaining {
        by_file.insert(file, lines);
    }

    let mut blocks = Vec::new();
    for (file, lines) in by_file {
        let mut lines = lines.iter();
        let Some(mut start) = lines.next() else {
            continue;
        };

        let mut end = start;
        for line in lines {
            if line == end + 1 {
                end = line;
            } else {
                blocks.push(UncoveredBlock {
                    file: file.to_string(),
                    start_line: start,
                    end_line: end,
                });
                start = line;
                end = line;
            }
        }
        blocks.push(UncoveredBlock {
            file: file.to_string(),
            start_line: start,
            end_line: end,
        });
    }

    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DiffStatus, FileDiff};
    use std::collections::HashSet;

    fn diff_with_lines(path: &str, lines: &[u32]) -> FileDiff {
        let mut changed_lines = RoaringBitmap::new();
        for line in lines {
            changed_lines.insert(*line);
        }
        FileDiff {
            path: path.to_string(),
            old_path: None,
            status: DiffStatus::Modified,
            changed_lines,
        }
    }

    fn sparse_rows(dense: &[Vec<Vec<u32>>], file_count: usize) -> Vec<SparseTestCoverageRow> {
        dense
            .iter()
            .map(|row| SparseTestCoverageRow::from_dense(row, file_count))
            .collect()
    }

    fn basic_sparse_map() -> TestMapIndex {
        let dense = vec![
            vec![vec![10, 11], vec![]],
            vec![vec![11], vec![20, 21]],
            vec![vec![10], vec![20]],
        ];
        TestMapIndex {
            tests: vec!["t1".to_string(), "t2".to_string(), "t3".to_string()],
            file_index: vec!["src/a.rs".to_string(), "src/b.rs".to_string()],
            sparse_coverage: sparse_rows(&dense, 2),
            ..TestMapIndex::default()
        }
    }

    #[test]
    fn test_select_impacted_tests_from_inverse_index() {
        let mut map = TestMapIndex::default();
        map.file_to_tests
            .entry("src/a.rs".to_string())
            .or_default()
            .insert("tests::a".to_string());

        let result = select_impacted_tests(&map, &[diff_with_lines("src/a.rs", &[])]);
        assert_eq!(result.selected_tests, vec!["tests::a".to_string()]);
        assert!(result.missing_mappings.is_empty());
    }

    #[test]
    fn test_select_impacted_tests_missing_mapping() {
        let map = TestMapIndex::default();
        let result = select_impacted_tests(&map, &[diff_with_lines("src/missing.rs", &[])]);
        assert!(result.selected_tests.is_empty());
        assert_eq!(result.missing_mappings, vec!["src/missing.rs".to_string()]);
    }

    #[test]
    fn test_plan_impacted_tests_greedy_and_coverage() {
        let map = basic_sparse_map();
        let diffs = vec![
            diff_with_lines("src/a.rs", &[10, 11, 12]),
            diff_with_lines("src/b.rs", &[20, 21]),
        ];

        let plan = plan_impacted_tests(&map, &diffs, 2, 0.8);
        assert_eq!(plan.changed_lines_total, 5);
        assert_eq!(plan.tests.len(), 2);
        assert_eq!(plan.tests[0].id, "t2");
        assert!(plan.plan_coverage_pct >= 0.8);
    }

    #[test]
    fn test_plan_impacted_tests_deterministic_tie_break() {
        let dense = vec![vec![vec![1]], vec![vec![1]]];
        let map = TestMapIndex {
            tests: vec!["aaa".to_string(), "bbb".to_string()],
            file_index: vec!["src/a.rs".to_string()],
            sparse_coverage: sparse_rows(&dense, 1),
            ..TestMapIndex::default()
        };
        let diffs = vec![diff_with_lines("src/a.rs", &[1])];

        let plan = plan_impacted_tests(&map, &diffs, 1, 1.0);
        assert_eq!(plan.tests[0].id, "aaa");
    }

    #[test]
    fn test_uncovered_blocks_compact_ranges() {
        let dense = vec![vec![vec![1, 2]]];
        let map = TestMapIndex {
            tests: vec!["t1".to_string()],
            file_index: vec!["src/a.rs".to_string()],
            sparse_coverage: sparse_rows(&dense, 1),
            ..TestMapIndex::default()
        };
        let diffs = vec![diff_with_lines("src/a.rs", &[1, 2, 3, 5])];

        let plan = plan_impacted_tests(&map, &diffs, 1, 1.0);
        assert_eq!(plan.uncovered_blocks.len(), 2);
        assert_eq!(plan.uncovered_blocks[0].start_line, 3);
        assert_eq!(plan.uncovered_blocks[0].end_line, 3);
        assert_eq!(plan.uncovered_blocks[1].start_line, 5);
    }

    #[test]
    fn legacy_dense_source_maps_remain_compatible() {
        let dense = vec![
            vec![vec![10, 11], vec![]],
            vec![vec![11], vec![20, 21]],
            vec![vec![10], vec![20]],
        ];
        let map = TestMapIndex {
            tests: vec!["t1".to_string(), "t2".to_string(), "t3".to_string()],
            file_index: vec!["src/a.rs".to_string(), "src/b.rs".to_string()],
            coverage: dense,
            ..TestMapIndex::default()
        };
        let diffs = vec![
            diff_with_lines("src/a.rs", &[10, 11, 12]),
            diff_with_lines("src/b.rs", &[20, 21]),
        ];

        assert_eq!(
            plan_impacted_tests(&map, &diffs, 2, 0.8),
            plan_impacted_tests(&basic_sparse_map(), &diffs, 2, 0.8)
        );
    }

    #[test]
    fn sparse_planner_matches_legacy_dense_planner_across_generated_cases() {
        for seed in 0..192_u64 {
            let mut rng = DeterministicRng::new(seed);
            let test_count = 1 + rng.usize(12);
            let file_count = 1 + rng.usize(10);
            let mut dense = vec![vec![Vec::new(); file_count]; test_count];

            for row in &mut dense {
                for cell in row {
                    if rng.usize(4) == 0 {
                        let line_count = 1 + rng.usize(8);
                        for _ in 0..line_count {
                            cell.push(1 + rng.usize(48) as u32);
                        }
                    }
                }
            }

            let tests = (0..test_count)
                .map(|idx| format!("test_{:03}", test_count - idx))
                .collect::<Vec<_>>();
            let file_index = (0..file_count)
                .map(|idx| format!("src/file_{idx:03}.rs"))
                .collect::<Vec<_>>();
            let sparse = TestMapIndex {
                tests: tests.clone(),
                file_index: file_index.clone(),
                sparse_coverage: sparse_rows(&dense, file_count),
                ..TestMapIndex::default()
            };
            let legacy = TestMapIndex {
                tests,
                file_index: file_index.clone(),
                coverage: dense,
                ..TestMapIndex::default()
            };

            let diff_count = 1 + rng.usize(file_count + 3);
            let mut diffs = Vec::with_capacity(diff_count);
            for diff_idx in 0..diff_count {
                let mapped = rng.usize(5) != 0;
                let mapped_file = rng.usize(file_count);
                let path = if mapped {
                    file_index[mapped_file].clone()
                } else {
                    format!("unmapped/file_{diff_idx:03}.rs")
                };
                let old_path = if !mapped && rng.usize(2) == 0 {
                    Some(file_index[mapped_file].clone())
                } else {
                    None
                };
                let mut changed_lines = RoaringBitmap::new();
                for _ in 0..(1 + rng.usize(16)) {
                    changed_lines.insert(1 + rng.usize(48) as u32);
                }
                diffs.push(FileDiff {
                    path,
                    old_path,
                    status: DiffStatus::Modified,
                    changed_lines,
                });
            }

            let max_tests = rng.usize(test_count + 2);
            let target_coverage = [0.0, 0.25, 0.5, 0.9, 1.0][rng.usize(5)];
            let expected = legacy_dense_plan(&legacy, &diffs, max_tests, target_coverage);
            let actual = plan_impacted_tests(&sparse, &diffs, max_tests, target_coverage);
            assert_eq!(actual, expected, "planner parity failed for seed {seed}");
        }
    }

    struct DeterministicRng(u64);

    impl DeterministicRng {
        fn new(seed: u64) -> Self {
            Self(seed ^ 0x9e37_79b9_7f4a_7c15)
        }

        fn usize(&mut self, upper_bound: usize) -> usize {
            debug_assert!(upper_bound > 0);
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 32) as usize) % upper_bound
        }
    }

    /// Frozen oracle for the pre-v3 dense planner. Keep this intentionally
    /// allocation-heavy so generated tests detect behavior drift in the
    /// sparse implementation.
    fn legacy_dense_plan(
        index: &TestMapIndex,
        diffs: &[FileDiff],
        max_tests: usize,
        target_coverage: f64,
    ) -> ImpactPlan {
        let mut plan = ImpactPlan {
            next_command: "covy impact run --plan plan.json -- <your-test-command-template>"
                .to_string(),
            ..Default::default()
        };
        let file_to_idx: BTreeMap<&str, usize> = index
            .file_index
            .iter()
            .enumerate()
            .map(|(idx, file)| (file.as_str(), idx))
            .collect();
        let mut mapped_remaining = BTreeMap::new();
        let mut mapped_file_names = BTreeMap::new();
        let mut unmapped_remaining = BTreeMap::new();
        for diff in diffs {
            let changed = diff.changed_lines.clone();
            if changed.is_empty() {
                continue;
            }
            let mapped_idx = file_to_idx.get(diff.path.as_str()).copied().or_else(|| {
                diff.old_path
                    .as_deref()
                    .and_then(|path| file_to_idx.get(path).copied())
            });
            if let Some(file_idx) = mapped_idx {
                mapped_file_names
                    .entry(file_idx)
                    .or_insert_with(|| index.file_index[file_idx].clone());
                mapped_remaining
                    .entry(file_idx)
                    .or_insert_with(RoaringBitmap::new)
                    .extend(changed.iter());
            } else {
                unmapped_remaining
                    .entry(diff.path.clone())
                    .or_insert_with(RoaringBitmap::new)
                    .extend(changed.iter());
            }
        }
        plan.changed_lines_total =
            total_bitmap_lines(&mapped_remaining) + total_bitmap_lines_by_name(&unmapped_remaining);
        if plan.changed_lines_total == 0 {
            plan.plan_coverage_pct = 1.0;
            return plan;
        }

        let original_mapped = mapped_remaining.clone();
        let test_rows = index
            .coverage
            .iter()
            .map(|row| {
                row.iter()
                    .map(|lines| lines.iter().copied().collect::<RoaringBitmap>())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let max_index_tests = index.tests.len().min(test_rows.len());
        let mut selected = HashSet::new();

        while plan.tests.len() < max_tests && selected.len() < max_index_tests {
            let mut best: Option<(usize, u64, u64, String)> = None;
            for test_idx in 0..max_index_tests {
                if selected.contains(&test_idx) {
                    continue;
                }
                let gain = legacy_gain(test_rows.get(test_idx), &mapped_remaining);
                if gain == 0 {
                    continue;
                }
                let overlap = legacy_gain(test_rows.get(test_idx), &original_mapped);
                let id = index.tests[test_idx].clone();
                best = match best {
                    None => Some((test_idx, gain, overlap, id)),
                    Some((best_idx, best_gain, best_overlap, best_id)) => {
                        if gain > best_gain
                            || (gain == best_gain && overlap > best_overlap)
                            || (gain == best_gain && overlap == best_overlap && id < best_id)
                        {
                            Some((test_idx, gain, overlap, id))
                        } else {
                            Some((best_idx, best_gain, best_overlap, best_id))
                        }
                    }
                };
            }
            let Some((winner_idx, winner_gain, winner_overlap, winner_id)) = best else {
                break;
            };
            selected.insert(winner_idx);
            legacy_subtract(test_rows.get(winner_idx), &mut mapped_remaining);
            plan.tests.push(PlannedTest {
                id: winner_id.clone(),
                name: winner_id,
                estimated_overlap_lines: winner_overlap,
                marginal_gain_lines: winner_gain,
            });
            let remaining_total = total_bitmap_lines(&mapped_remaining)
                + total_bitmap_lines_by_name(&unmapped_remaining);
            plan.changed_lines_covered_by_plan =
                plan.changed_lines_total.saturating_sub(remaining_total);
            plan.plan_coverage_pct =
                plan.changed_lines_covered_by_plan as f64 / plan.changed_lines_total as f64;
            if plan.plan_coverage_pct >= target_coverage {
                break;
            }
        }
        let remaining_total =
            total_bitmap_lines(&mapped_remaining) + total_bitmap_lines_by_name(&unmapped_remaining);
        plan.changed_lines_covered_by_plan =
            plan.changed_lines_total.saturating_sub(remaining_total);
        plan.plan_coverage_pct =
            plan.changed_lines_covered_by_plan as f64 / plan.changed_lines_total as f64;
        plan.uncovered_blocks =
            build_uncovered_blocks(&mapped_remaining, &mapped_file_names, &unmapped_remaining);
        plan
    }

    fn legacy_gain(
        test_row: Option<&Vec<RoaringBitmap>>,
        mapped_remaining: &BTreeMap<usize, RoaringBitmap>,
    ) -> u64 {
        let Some(test_row) = test_row else {
            return 0;
        };
        mapped_remaining
            .iter()
            .filter_map(|(file_idx, remaining)| {
                test_row
                    .get(*file_idx)
                    .map(|test_lines| (&remaining.clone() & test_lines).len())
            })
            .sum()
    }

    fn legacy_subtract(
        test_row: Option<&Vec<RoaringBitmap>>,
        mapped_remaining: &mut BTreeMap<usize, RoaringBitmap>,
    ) {
        let Some(test_row) = test_row else {
            return;
        };
        let keys = mapped_remaining.keys().copied().collect::<Vec<_>>();
        for file_idx in keys {
            if let (Some(test_lines), Some(remaining)) =
                (test_row.get(file_idx), mapped_remaining.get_mut(&file_idx))
            {
                *remaining -= test_lines;
                if remaining.is_empty() {
                    mapped_remaining.remove(&file_idx);
                }
            }
        }
    }
}
