use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use roaring::RoaringBitmap;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TestMapMetadata {
    pub schema_version: u16,
    pub path_norm_version: u16,
    pub repo_root_id: Option<String>,
    pub generated_at: u64,
    pub granularity: String,
    #[serde(default)]
    pub commit_sha: Option<String>,
    #[serde(default)]
    pub created_at: Option<u64>,
    #[serde(default)]
    pub toolchain_fingerprint: Option<String>,
}

impl Default for TestMapMetadata {
    fn default() -> Self {
        Self {
            schema_version: 3,
            path_norm_version: 1,
            repo_root_id: None,
            generated_at: 0,
            granularity: "file".to_string(),
            commit_sha: None,
            created_at: None,
            toolchain_fingerprint: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default, PartialEq)]
pub struct SparseFileCoverage {
    /// Index into [`TestMapIndex::file_index`].
    pub file_idx: usize,
    /// Covered source lines encoded as a compressed bitmap.
    #[serde(with = "roaring_bitmap_serde")]
    pub lines: RoaringBitmap,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default, PartialEq)]
pub struct SparseTestCoverageRow {
    /// Sorted, unique, non-empty coverage cells keyed by `file_idx`.
    pub files: Vec<SparseFileCoverage>,
}

impl SparseTestCoverageRow {
    /// Convert one historical dense row, discarding empty and out-of-range
    /// cells while deduplicating line numbers.
    pub fn from_dense(row: &[Vec<u32>], file_count: usize) -> Self {
        let files = row
            .iter()
            .take(file_count)
            .enumerate()
            .filter_map(|(file_idx, lines)| {
                if lines.is_empty() {
                    return None;
                }
                Some(SparseFileCoverage {
                    file_idx,
                    lines: lines.iter().copied().collect(),
                })
            })
            .collect();
        Self { files }
    }

    /// Return the covered lines for one canonical file index.
    pub fn lines_for_file(&self, file_idx: usize) -> Option<&RoaringBitmap> {
        let cell_idx = self
            .files
            .binary_search_by_key(&file_idx, |cell| cell.file_idx)
            .ok()?;
        Some(&self.files[cell_idx].lines)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct TestMapIndex {
    pub metadata: TestMapMetadata,
    pub test_language: BTreeMap<String, String>,
    /// Legacy index used by pre-v2 impact planners (file-level only).
    pub test_to_files: BTreeMap<String, BTreeSet<String>>,
    /// Legacy inverse index used by pre-v2 impact planners (file-level only).
    pub file_to_tests: BTreeMap<String, BTreeSet<String>>,
    /// Canonical test id list (index -> test id), introduced in schema v2.
    #[serde(default)]
    pub tests: Vec<String>,
    /// Canonical file key list (index -> repo-relative path), introduced in
    /// schema v2.
    #[serde(default)]
    pub file_index: Vec<String>,
    /// V3 sparse coverage rows: test index -> sorted non-empty file coverage.
    #[serde(default)]
    pub sparse_coverage: Vec<SparseTestCoverageRow>,
    /// V2 dense coverage retained only for in-memory/source compatibility.
    ///
    /// V3 persistence converts this field to `sparse_coverage` and does not
    /// write the dense matrix.
    #[serde(default)]
    pub coverage: Vec<Vec<Vec<u32>>>,
}

impl TestMapIndex {
    /// Whether the map contains indexed line-level coverage rather than only
    /// the legacy file-level forward and inverse maps.
    pub fn has_line_coverage(&self) -> bool {
        !self.tests.is_empty()
            && !self.file_index.is_empty()
            && (!self.sparse_coverage.is_empty() || !self.coverage.is_empty())
    }

    /// Borrow v3 sparse rows, or lazily convert an in-memory v2 dense map.
    ///
    /// When both representations are present, the v3 sparse rows are
    /// authoritative. Persisted v2 maps are migrated during deserialization,
    /// so the owned conversion is limited to source/API compatibility.
    pub fn sparse_coverage_rows(&self) -> Cow<'_, [SparseTestCoverageRow]> {
        if !self.sparse_coverage.is_empty() || self.coverage.is_empty() {
            return Cow::Borrowed(&self.sparse_coverage);
        }

        Cow::Owned(
            (0..self.tests.len())
                .map(|test_idx| {
                    self.coverage
                        .get(test_idx)
                        .map(|row| SparseTestCoverageRow::from_dense(row, self.file_index.len()))
                        .unwrap_or_default()
                })
                .collect(),
        )
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct TestTimingHistory {
    pub generated_at: u64,
    pub duration_ms: BTreeMap<String, u64>,
    pub sample_count: BTreeMap<String, u32>,
    pub last_seen: BTreeMap<String, u64>,
}

mod roaring_bitmap_serde {
    use super::*;

    pub fn serialize<S>(bitmap: &RoaringBitmap, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut bytes = Vec::with_capacity(bitmap.serialized_size());
        bitmap
            .serialize_into(&mut bytes)
            .map_err(serde::ser::Error::custom)?;
        serializer.serialize_bytes(&bytes)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<RoaringBitmap, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes = <Vec<u8> as serde::Deserialize>::deserialize(deserializer)?;
        RoaringBitmap::deserialize_from(Cursor::new(bytes)).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_rows_convert_to_sorted_non_empty_bitmaps() {
        let row = vec![vec![5, 1, 5], Vec::new(), vec![9], vec![99]];

        let sparse = SparseTestCoverageRow::from_dense(&row, 3);

        assert_eq!(
            sparse
                .files
                .iter()
                .map(|cell| cell.file_idx)
                .collect::<Vec<_>>(),
            vec![0, 2]
        );
        assert_eq!(sparse.files[0].lines.iter().collect::<Vec<_>>(), vec![1, 5]);
        assert_eq!(sparse.files[1].lines.iter().collect::<Vec<_>>(), vec![9]);
    }

    #[test]
    fn v3_rows_are_borrowed_and_authoritative() {
        let sparse_row = SparseTestCoverageRow {
            files: vec![SparseFileCoverage {
                file_idx: 0,
                lines: [7].into_iter().collect(),
            }],
        };
        let index = TestMapIndex {
            tests: vec!["test".to_string()],
            file_index: vec!["src/lib.rs".to_string()],
            sparse_coverage: vec![sparse_row],
            coverage: vec![vec![vec![99]]],
            ..TestMapIndex::default()
        };

        let rows = index.sparse_coverage_rows();
        assert!(matches!(rows, Cow::Borrowed(_)));
        assert!(rows[0].lines_for_file(0).unwrap().contains(7));
        assert!(!rows[0].lines_for_file(0).unwrap().contains(99));
    }

    #[test]
    fn v2_dense_rows_are_lazily_converted_for_source_compatibility() {
        let index = TestMapIndex {
            tests: vec!["test".to_string()],
            file_index: vec!["src/lib.rs".to_string()],
            coverage: vec![vec![vec![2, 3]]],
            ..TestMapIndex::default()
        };

        assert!(index.has_line_coverage());
        let rows = index.sparse_coverage_rows();
        assert!(matches!(rows, Cow::Owned(_)));
        assert_eq!(
            rows[0]
                .lines_for_file(0)
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
    }
}
