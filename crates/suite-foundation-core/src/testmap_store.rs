use std::io::Cursor;

use roaring::RoaringBitmap;

use crate::error::CovyError;
use crate::testmap::{SparseTestCoverageRow, TestMapIndex, TestMapMetadata};

pub const TESTMAP_SCHEMA_VERSION: u16 = 3;

const TESTMAP_MAGIC: &[u8; 7] = b"P28TMAP";
const TESTMAP_HEADER_LEN: usize = TESTMAP_MAGIC.len() + std::mem::size_of::<u16>();

#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, wincode::SchemaRead, wincode::SchemaWrite,
)]
struct LegacyTestMapMetadataV1 {
    schema_version: u16,
    path_norm_version: u16,
    repo_root_id: Option<String>,
    generated_at: u64,
    granularity: String,
}

#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, wincode::SchemaRead, wincode::SchemaWrite,
)]
struct LegacyTestMapIndexV1 {
    metadata: LegacyTestMapMetadataV1,
    test_language: std::collections::BTreeMap<String, String>,
    test_to_files: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    file_to_tests: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
}

#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, wincode::SchemaRead, wincode::SchemaWrite,
)]
struct LegacyTestMapMetadataV2 {
    schema_version: u16,
    path_norm_version: u16,
    repo_root_id: Option<String>,
    generated_at: u64,
    granularity: String,
    commit_sha: Option<String>,
    created_at: Option<u64>,
    toolchain_fingerprint: Option<String>,
}

#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, wincode::SchemaRead, wincode::SchemaWrite,
)]
struct LegacyTestMapIndexV2 {
    metadata: LegacyTestMapMetadataV2,
    test_language: std::collections::BTreeMap<String, String>,
    test_to_files: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    file_to_tests: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    tests: Vec<String>,
    file_index: Vec<String>,
    coverage: Vec<Vec<Vec<u32>>>,
}

#[derive(
    Debug, serde::Serialize, serde::Deserialize, wincode::SchemaRead, wincode::SchemaWrite,
)]
struct StoredTestMapMetadataV3 {
    schema_version: u16,
    path_norm_version: u16,
    repo_root_id: Option<String>,
    generated_at: u64,
    granularity: String,
    commit_sha: Option<String>,
    created_at: Option<u64>,
    toolchain_fingerprint: Option<String>,
}

impl StoredTestMapMetadataV3 {
    fn from_current(metadata: &TestMapMetadata) -> Self {
        Self {
            schema_version: metadata.schema_version,
            path_norm_version: metadata.path_norm_version,
            repo_root_id: metadata.repo_root_id.clone(),
            generated_at: metadata.generated_at,
            granularity: metadata.granularity.clone(),
            commit_sha: metadata.commit_sha.clone(),
            created_at: metadata.created_at,
            toolchain_fingerprint: metadata.toolchain_fingerprint.clone(),
        }
    }

    fn into_current(self) -> TestMapMetadata {
        TestMapMetadata {
            schema_version: self.schema_version,
            path_norm_version: self.path_norm_version,
            repo_root_id: self.repo_root_id,
            generated_at: self.generated_at,
            granularity: self.granularity,
            commit_sha: self.commit_sha,
            created_at: self.created_at,
            toolchain_fingerprint: self.toolchain_fingerprint,
        }
    }
}

#[derive(
    Debug, serde::Serialize, serde::Deserialize, wincode::SchemaRead, wincode::SchemaWrite,
)]
struct StoredSparseFileCoverageV3 {
    file_idx: u64,
    lines: Vec<u8>,
}

#[derive(
    Debug, serde::Serialize, serde::Deserialize, wincode::SchemaRead, wincode::SchemaWrite,
)]
struct StoredSparseTestCoverageRowV3 {
    files: Vec<StoredSparseFileCoverageV3>,
}

#[derive(Debug, serde::Serialize, wincode::SchemaWrite)]
struct StoredTestMapIndexV3Ref<'a> {
    metadata: StoredTestMapMetadataV3,
    test_language: &'a std::collections::BTreeMap<String, String>,
    test_to_files: &'a std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    file_to_tests: &'a std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    tests: &'a [String],
    file_index: &'a [String],
    sparse_coverage: &'a [StoredSparseTestCoverageRowV3],
}

#[derive(Debug, serde::Deserialize, wincode::SchemaRead)]
struct StoredTestMapIndexV3 {
    metadata: StoredTestMapMetadataV3,
    test_language: std::collections::BTreeMap<String, String>,
    test_to_files: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    file_to_tests: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    tests: Vec<String>,
    file_index: Vec<String>,
    sparse_coverage: Vec<StoredSparseTestCoverageRowV3>,
}

/// Serialize TestMapIndex to bytes for storage.
pub fn serialize_testmap(index: &TestMapIndex) -> Result<Vec<u8>, CovyError> {
    let sparse_coverage = index.sparse_coverage_rows();
    validate_sparse_coverage(&index.tests, &index.file_index, &sparse_coverage)?;
    let stored_sparse_coverage = encode_sparse_coverage(&sparse_coverage)?;

    let mut metadata = index.metadata.clone();
    metadata.schema_version = TESTMAP_SCHEMA_VERSION;
    let stored = StoredTestMapIndexV3Ref {
        metadata: StoredTestMapMetadataV3::from_current(&metadata),
        test_language: &index.test_language,
        test_to_files: &index.test_to_files,
        file_to_tests: &index.file_to_tests,
        tests: &index.tests,
        file_index: &index.file_index,
        sparse_coverage: &stored_sparse_coverage,
    };
    let payload = wincode::serialize(&stored)
        .map_err(|error| CovyError::Cache(format!("Failed to serialize testmap: {error}")))?;
    let mut bytes = Vec::with_capacity(TESTMAP_HEADER_LEN + payload.len());
    bytes.extend_from_slice(TESTMAP_MAGIC);
    bytes.extend_from_slice(&TESTMAP_SCHEMA_VERSION.to_le_bytes());
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

/// Deserialize TestMapIndex from bytes.
pub fn deserialize_testmap(data: &[u8]) -> Result<TestMapIndex, CovyError> {
    if data.starts_with(TESTMAP_MAGIC) {
        return deserialize_v3_testmap(data);
    }

    if let Ok(stored) = wincode::deserialize::<LegacyTestMapIndexV2>(data) {
        return match stored.metadata.schema_version {
            2 => migrate_v2_testmap(stored),
            1 => Ok(normalize_struct_v1_testmap(stored)),
            version => Err(unsupported_version(version)),
        };
    }

    let legacy: LegacyTestMapIndexV1 = wincode::deserialize(data)
        .map_err(|error| CovyError::Cache(format!("Failed to deserialize testmap: {error}")))?;
    if legacy.metadata.schema_version != 1 {
        return Err(unsupported_version(legacy.metadata.schema_version));
    }
    Ok(normalize_v1_testmap(legacy))
}

fn deserialize_v3_testmap(data: &[u8]) -> Result<TestMapIndex, CovyError> {
    if data.len() < TESTMAP_HEADER_LEN {
        return Err(CovyError::Cache(
            "Failed to deserialize testmap: truncated v3 header".to_string(),
        ));
    }
    let version_offset = TESTMAP_MAGIC.len();
    let version = u16::from_le_bytes([data[version_offset], data[version_offset + 1]]);
    if version != TESTMAP_SCHEMA_VERSION {
        return Err(unsupported_version(version));
    }
    let stored: StoredTestMapIndexV3 = wincode::deserialize(&data[TESTMAP_HEADER_LEN..])
        .map_err(|error| CovyError::Cache(format!("Failed to deserialize testmap: {error}")))?;
    if stored.metadata.schema_version != TESTMAP_SCHEMA_VERSION {
        return Err(CovyError::Cache(format!(
            "Testmap header schema {version} disagrees with payload schema {}",
            stored.metadata.schema_version
        )));
    }
    let sparse_coverage = decode_sparse_coverage(stored.sparse_coverage)?;
    validate_sparse_coverage(&stored.tests, &stored.file_index, &sparse_coverage)?;
    Ok(TestMapIndex {
        metadata: stored.metadata.into_current(),
        test_language: stored.test_language,
        test_to_files: stored.test_to_files,
        file_to_tests: stored.file_to_tests,
        tests: stored.tests,
        file_index: stored.file_index,
        sparse_coverage,
        coverage: Vec::new(),
    })
}

fn encode_sparse_coverage(
    rows: &[SparseTestCoverageRow],
) -> Result<Vec<StoredSparseTestCoverageRowV3>, CovyError> {
    rows.iter()
        .enumerate()
        .map(|(test_idx, row)| {
            let files = row
                .files
                .iter()
                .enumerate()
                .map(|(cell_idx, cell)| {
                    let file_idx = u64::try_from(cell.file_idx).map_err(|error| {
                        CovyError::Cache(format!(
                            "Failed to serialize testmap file index at test row {test_idx}, file cell {cell_idx}: {error}"
                        ))
                    })?;
                    let mut lines = Vec::with_capacity(cell.lines.serialized_size());
                    cell.lines.serialize_into(&mut lines).map_err(|error| {
                        CovyError::Cache(format!(
                            "Failed to serialize testmap bitmap at test row {test_idx}, file cell {cell_idx}: {error}"
                        ))
                    })?;
                    Ok(StoredSparseFileCoverageV3 { file_idx, lines })
                })
                .collect::<Result<Vec<_>, CovyError>>()?;
            Ok(StoredSparseTestCoverageRowV3 { files })
        })
        .collect()
}

fn decode_sparse_coverage(
    rows: Vec<StoredSparseTestCoverageRowV3>,
) -> Result<Vec<SparseTestCoverageRow>, CovyError> {
    rows.into_iter()
        .enumerate()
        .map(|(test_idx, row)| {
            let files = row
                .files
                .into_iter()
                .enumerate()
                .map(|(cell_idx, cell)| {
                    let file_idx = usize::try_from(cell.file_idx).map_err(|error| {
                        CovyError::Cache(format!(
                            "Invalid sparse testmap: file index {} at test row {test_idx}, file cell {cell_idx} does not fit this platform: {error}",
                            cell.file_idx
                        ))
                    })?;
                    let mut cursor = Cursor::new(cell.lines.as_slice());
                    let lines =
                        RoaringBitmap::deserialize_from(&mut cursor).map_err(|error| {
                            CovyError::Cache(format!(
                                "Failed to deserialize testmap bitmap at test row {test_idx}, file cell {cell_idx}: {error}"
                            ))
                        })?;
                    if cursor.position() != cell.lines.len() as u64 {
                        return Err(CovyError::Cache(format!(
                            "Failed to deserialize testmap bitmap at test row {test_idx}, file cell {cell_idx}: trailing bytes"
                        )));
                    }
                    Ok(crate::testmap::SparseFileCoverage { file_idx, lines })
                })
                .collect::<Result<Vec<_>, CovyError>>()?;
            Ok(SparseTestCoverageRow { files })
        })
        .collect()
}

fn migrate_v2_testmap(stored: LegacyTestMapIndexV2) -> Result<TestMapIndex, CovyError> {
    let sparse_coverage = if stored.coverage.is_empty() {
        Vec::new()
    } else {
        (0..stored.tests.len())
            .map(|test_idx| {
                stored
                    .coverage
                    .get(test_idx)
                    .map(|row| SparseTestCoverageRow::from_dense(row, stored.file_index.len()))
                    .unwrap_or_default()
            })
            .collect()
    };
    validate_sparse_coverage(&stored.tests, &stored.file_index, &sparse_coverage)?;
    let mut metadata = TestMapMetadata {
        schema_version: stored.metadata.schema_version,
        path_norm_version: stored.metadata.path_norm_version,
        repo_root_id: stored.metadata.repo_root_id,
        generated_at: stored.metadata.generated_at,
        granularity: stored.metadata.granularity,
        commit_sha: stored.metadata.commit_sha,
        created_at: stored.metadata.created_at,
        toolchain_fingerprint: stored.metadata.toolchain_fingerprint,
    };
    metadata.schema_version = TESTMAP_SCHEMA_VERSION;
    Ok(TestMapIndex {
        metadata,
        test_language: stored.test_language,
        test_to_files: stored.test_to_files,
        file_to_tests: stored.file_to_tests,
        tests: stored.tests,
        file_index: stored.file_index,
        sparse_coverage,
        coverage: Vec::new(),
    })
}

fn normalize_v1_testmap(legacy: LegacyTestMapIndexV1) -> TestMapIndex {
    TestMapIndex {
        metadata: crate::testmap::TestMapMetadata {
            schema_version: legacy.metadata.schema_version,
            path_norm_version: legacy.metadata.path_norm_version,
            repo_root_id: legacy.metadata.repo_root_id,
            generated_at: legacy.metadata.generated_at,
            granularity: legacy.metadata.granularity,
            commit_sha: None,
            created_at: None,
            toolchain_fingerprint: None,
        },
        test_language: legacy.test_language,
        test_to_files: legacy.test_to_files,
        file_to_tests: legacy.file_to_tests,
        tests: Vec::new(),
        file_index: Vec::new(),
        sparse_coverage: Vec::new(),
        coverage: Vec::new(),
    }
}

fn normalize_struct_v1_testmap(index: LegacyTestMapIndexV2) -> TestMapIndex {
    TestMapIndex {
        metadata: crate::testmap::TestMapMetadata {
            schema_version: index.metadata.schema_version,
            path_norm_version: index.metadata.path_norm_version,
            repo_root_id: index.metadata.repo_root_id,
            generated_at: index.metadata.generated_at,
            granularity: index.metadata.granularity,
            commit_sha: None,
            created_at: None,
            toolchain_fingerprint: None,
        },
        test_language: index.test_language,
        test_to_files: index.test_to_files,
        file_to_tests: index.file_to_tests,
        tests: Vec::new(),
        file_index: Vec::new(),
        sparse_coverage: Vec::new(),
        coverage: Vec::new(),
    }
}

fn validate_sparse_coverage(
    tests: &[String],
    file_index: &[String],
    rows: &[SparseTestCoverageRow],
) -> Result<(), CovyError> {
    if !rows.is_empty() && rows.len() != tests.len() {
        return Err(CovyError::Cache(format!(
            "Invalid sparse testmap: {} test rows for {} tests",
            rows.len(),
            tests.len()
        )));
    }
    for (test_idx, row) in rows.iter().enumerate() {
        let mut previous_file_idx = None;
        for cell in &row.files {
            if cell.file_idx >= file_index.len() {
                return Err(CovyError::Cache(format!(
                    "Invalid sparse testmap: test row {test_idx} references file index {} but only {} files exist",
                    cell.file_idx,
                    file_index.len()
                )));
            }
            if previous_file_idx.is_some_and(|previous| previous >= cell.file_idx) {
                return Err(CovyError::Cache(format!(
                    "Invalid sparse testmap: test row {test_idx} file indexes are not strictly increasing"
                )));
            }
            if cell.lines.is_empty() {
                return Err(CovyError::Cache(format!(
                    "Invalid sparse testmap: test row {test_idx} contains an empty coverage cell"
                )));
            }
            previous_file_idx = Some(cell.file_idx);
        }
    }
    Ok(())
}

fn unsupported_version(version: u16) -> CovyError {
    CovyError::Cache(format!(
        "Unsupported testmap schema version {version} (expected {TESTMAP_SCHEMA_VERSION}, 2, or 1)"
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use roaring::RoaringBitmap;

    use crate::testmap::SparseFileCoverage;

    use super::*;

    fn bitmap(lines: &[u32]) -> RoaringBitmap {
        lines.iter().copied().collect()
    }

    fn sample_sparse_index() -> TestMapIndex {
        let test_a = "crate::tests::alpha".to_string();
        let test_b = "crate::tests::beta".to_string();
        let file_a = "src/alpha.rs".to_string();
        let file_b = "src/beta.rs".to_string();
        let file_c = "src/shared.rs".to_string();

        TestMapIndex {
            metadata: TestMapMetadata {
                schema_version: 2,
                path_norm_version: 1,
                repo_root_id: Some("repo-id".to_string()),
                generated_at: 123,
                granularity: "line".to_string(),
                commit_sha: Some("abc123".to_string()),
                created_at: Some(456),
                toolchain_fingerprint: Some("rustc-test".to_string()),
            },
            test_language: BTreeMap::from([
                (test_a.clone(), "rust".to_string()),
                (test_b.clone(), "rust".to_string()),
            ]),
            test_to_files: BTreeMap::from([
                (
                    test_a.clone(),
                    BTreeSet::from([file_a.clone(), file_c.clone()]),
                ),
                (test_b.clone(), BTreeSet::from([file_b.clone()])),
            ]),
            file_to_tests: BTreeMap::from([
                (file_a.clone(), BTreeSet::from([test_a.clone()])),
                (file_b.clone(), BTreeSet::from([test_b.clone()])),
                (file_c.clone(), BTreeSet::from([test_a.clone()])),
            ]),
            tests: vec![test_a, test_b],
            file_index: vec![file_a, file_b, file_c],
            sparse_coverage: vec![
                SparseTestCoverageRow {
                    files: vec![
                        SparseFileCoverage {
                            file_idx: 0,
                            lines: bitmap(&[1, 4]),
                        },
                        SparseFileCoverage {
                            file_idx: 2,
                            lines: bitmap(&[9]),
                        },
                    ],
                },
                SparseTestCoverageRow {
                    files: vec![SparseFileCoverage {
                        file_idx: 1,
                        lines: bitmap(&[2, 8]),
                    }],
                },
            ],
            coverage: Vec::new(),
        }
    }

    fn sample_legacy_v2(schema_version: u16) -> LegacyTestMapIndexV2 {
        let index = sample_sparse_index();
        LegacyTestMapIndexV2 {
            metadata: LegacyTestMapMetadataV2 {
                schema_version,
                path_norm_version: index.metadata.path_norm_version,
                repo_root_id: index.metadata.repo_root_id,
                generated_at: index.metadata.generated_at,
                granularity: index.metadata.granularity,
                commit_sha: index.metadata.commit_sha,
                created_at: index.metadata.created_at,
                toolchain_fingerprint: index.metadata.toolchain_fingerprint,
            },
            test_language: index.test_language,
            test_to_files: index.test_to_files,
            file_to_tests: index.file_to_tests,
            tests: index.tests,
            file_index: index.file_index,
            coverage: vec![
                vec![vec![5, 1, 5, 3], Vec::new(), vec![9]],
                vec![Vec::new(), vec![8, 2], Vec::new()],
            ],
        }
    }

    fn encode_v3_unchecked(
        index: &TestMapIndex,
        header_version: u16,
        payload_version: u16,
    ) -> Vec<u8> {
        let sparse_coverage = encode_sparse_coverage(&index.sparse_coverage).unwrap();
        encode_v3_with_stored_rows(index, &sparse_coverage, header_version, payload_version)
    }

    fn encode_v3_with_stored_rows(
        index: &TestMapIndex,
        sparse_coverage: &[StoredSparseTestCoverageRowV3],
        header_version: u16,
        payload_version: u16,
    ) -> Vec<u8> {
        let mut metadata = index.metadata.clone();
        metadata.schema_version = payload_version;
        let stored = StoredTestMapIndexV3Ref {
            metadata: StoredTestMapMetadataV3::from_current(&metadata),
            test_language: &index.test_language,
            test_to_files: &index.test_to_files,
            file_to_tests: &index.file_to_tests,
            tests: &index.tests,
            file_index: &index.file_index,
            sparse_coverage,
        };
        let payload = wincode::serialize(&stored).unwrap();
        let mut bytes = Vec::with_capacity(TESTMAP_HEADER_LEN + payload.len());
        bytes.extend_from_slice(TESTMAP_MAGIC);
        bytes.extend_from_slice(&header_version.to_le_bytes());
        bytes.extend_from_slice(&payload);
        bytes
    }

    #[test]
    fn serialize_v3_should_prefix_magic_and_explicit_version() {
        let index = sample_sparse_index();
        let bytes = serialize_testmap(&index).unwrap();

        assert_eq!(
            (
                &bytes[..TESTMAP_MAGIC.len()],
                u16::from_le_bytes([bytes[TESTMAP_MAGIC.len()], bytes[TESTMAP_MAGIC.len() + 1]])
            ),
            (TESTMAP_MAGIC.as_slice(), TESTMAP_SCHEMA_VERSION)
        );
    }

    #[test]
    fn serialize_v3_should_be_deterministic() {
        let index = sample_sparse_index();

        assert_eq!(
            serialize_testmap(&index).unwrap(),
            serialize_testmap(&index).unwrap()
        );
    }

    #[test]
    fn v3_wire_format_should_match_golden_digest() {
        let bytes = serialize_testmap(&sample_sparse_index()).unwrap();

        assert_eq!(
            blake3::hash(&bytes).to_hex().as_str(),
            "9fb1945c7764b47ba21a6d88e775d1d9137f96c1bf3650c9a6c7e7230267e700"
        );
    }

    #[test]
    fn v3_roundtrip_should_preserve_sparse_coverage_without_dense_matrix() {
        let index = sample_sparse_index();
        let bytes = serialize_testmap(&index).unwrap();
        let restored = deserialize_testmap(&bytes).unwrap();
        let mut expected_metadata = index.metadata.clone();
        expected_metadata.schema_version = TESTMAP_SCHEMA_VERSION;

        assert_eq!(restored.metadata, expected_metadata);
        assert_eq!(restored.test_language, index.test_language);
        assert_eq!(restored.test_to_files, index.test_to_files);
        assert_eq!(restored.file_to_tests, index.file_to_tests);
        assert_eq!(restored.tests, index.tests);
        assert_eq!(restored.file_index, index.file_index);
        assert_eq!(restored.sparse_coverage, index.sparse_coverage);
        assert!(restored.coverage.is_empty());
    }

    #[test]
    fn serialize_v3_should_convert_in_memory_dense_compatibility_rows() {
        let mut index = sample_sparse_index();
        index.sparse_coverage.clear();
        index.coverage = vec![
            vec![vec![4, 1, 4], Vec::new(), vec![9]],
            vec![Vec::new(), vec![8, 2], Vec::new()],
        ];

        let restored = deserialize_testmap(&serialize_testmap(&index).unwrap()).unwrap();

        assert_eq!(
            restored.sparse_coverage,
            vec![
                SparseTestCoverageRow {
                    files: vec![
                        SparseFileCoverage {
                            file_idx: 0,
                            lines: bitmap(&[1, 4]),
                        },
                        SparseFileCoverage {
                            file_idx: 2,
                            lines: bitmap(&[9]),
                        },
                    ],
                },
                SparseTestCoverageRow {
                    files: vec![SparseFileCoverage {
                        file_idx: 1,
                        lines: bitmap(&[2, 8]),
                    }],
                },
            ]
        );
        assert!(restored.coverage.is_empty());
    }

    #[test]
    fn deserialize_exact_v2_should_migrate_dense_rows_to_v3_sparse_rows() {
        let legacy = sample_legacy_v2(2);
        let bytes = wincode::serialize(&legacy).unwrap();

        let restored = deserialize_testmap(&bytes).unwrap();

        assert_eq!(restored.metadata.schema_version, TESTMAP_SCHEMA_VERSION);
        assert_eq!(restored.metadata.commit_sha.as_deref(), Some("abc123"));
        assert_eq!(
            restored.sparse_coverage,
            vec![
                SparseTestCoverageRow {
                    files: vec![
                        SparseFileCoverage {
                            file_idx: 0,
                            lines: bitmap(&[1, 3, 5]),
                        },
                        SparseFileCoverage {
                            file_idx: 2,
                            lines: bitmap(&[9]),
                        },
                    ],
                },
                SparseTestCoverageRow {
                    files: vec![SparseFileCoverage {
                        file_idx: 1,
                        lines: bitmap(&[2, 8]),
                    }],
                },
            ]
        );
        assert!(restored.coverage.is_empty());
    }

    #[test]
    fn deserialize_exact_short_v1_should_preserve_file_maps() {
        let legacy = LegacyTestMapIndexV1 {
            metadata: LegacyTestMapMetadataV1 {
                schema_version: 1,
                path_norm_version: 1,
                repo_root_id: Some("deadbeef".to_string()),
                generated_at: 123,
                granularity: "file".to_string(),
            },
            test_language: {
                let mut m = std::collections::BTreeMap::new();
                m.insert("com.foo.BarTest".to_string(), "java".to_string());
                m
            },
            test_to_files: {
                let mut m = std::collections::BTreeMap::new();
                m.entry("com.foo.BarTest".to_string())
                    .or_insert_with(std::collections::BTreeSet::new)
                    .insert("src/main/java/com/foo/Bar.java".to_string());
                m
            },
            file_to_tests: {
                let mut m = std::collections::BTreeMap::new();
                m.entry("src/main/java/com/foo/Bar.java".to_string())
                    .or_insert_with(std::collections::BTreeSet::new)
                    .insert("com.foo.BarTest".to_string());
                m
            },
        };

        let bytes = wincode::serialize(&legacy).unwrap();
        let restored = deserialize_testmap(&bytes).unwrap();

        assert_eq!(restored.metadata.schema_version, 1);
        assert_eq!(
            restored
                .test_to_files
                .get("com.foo.BarTest")
                .map(|s| s.len())
                .unwrap_or_default(),
            1
        );
        assert!(restored.tests.is_empty());
        assert!(restored.file_index.is_empty());
        assert!(restored.sparse_coverage.is_empty());
        assert!(restored.coverage.is_empty());
    }

    #[test]
    fn deserialize_exact_struct_v1_should_remove_unversioned_line_fields() {
        let legacy = sample_legacy_v2(1);
        let expected_test_to_files = legacy.test_to_files.clone();
        let expected_file_to_tests = legacy.file_to_tests.clone();
        let bytes = wincode::serialize(&legacy).unwrap();

        let restored = deserialize_testmap(&bytes).unwrap();

        assert_eq!(restored.metadata.schema_version, 1);
        assert!(restored.metadata.commit_sha.is_none());
        assert!(restored.metadata.created_at.is_none());
        assert!(restored.metadata.toolchain_fingerprint.is_none());
        assert!(restored.tests.is_empty());
        assert!(restored.file_index.is_empty());
        assert!(restored.sparse_coverage.is_empty());
        assert!(restored.coverage.is_empty());
        assert_eq!(restored.test_to_files, expected_test_to_files);
        assert_eq!(restored.file_to_tests, expected_file_to_tests);
    }

    #[test]
    fn deserialize_v3_should_reject_future_header_version() {
        let bytes = encode_v3_unchecked(&sample_sparse_index(), 4, TESTMAP_SCHEMA_VERSION);

        let error = deserialize_testmap(&bytes).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Unsupported testmap schema version 4"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn deserialize_v3_should_reject_header_payload_version_disagreement() {
        let bytes = encode_v3_unchecked(&sample_sparse_index(), TESTMAP_SCHEMA_VERSION, 2);

        let error = deserialize_testmap(&bytes).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("header schema 3 disagrees with payload schema 2"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn deserialize_v3_should_reject_truncated_header() {
        let error = deserialize_testmap(TESTMAP_MAGIC).unwrap_err();

        assert!(
            error.to_string().contains("truncated v3 header"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn deserialize_v3_should_reject_trailing_payload_bytes() {
        let mut bytes = serialize_testmap(&sample_sparse_index()).unwrap();
        bytes.push(0);

        assert!(deserialize_testmap(&bytes).is_err());
    }

    #[test]
    fn deserialize_v2_should_reject_trailing_payload_bytes() {
        let mut bytes = wincode::serialize(&sample_legacy_v2(2)).unwrap();
        bytes.push(0);

        assert!(deserialize_testmap(&bytes).is_err());
    }

    #[test]
    fn deserialize_short_v1_should_reject_trailing_payload_bytes() {
        let legacy = LegacyTestMapIndexV1 {
            metadata: LegacyTestMapMetadataV1 {
                schema_version: 1,
                path_norm_version: 1,
                repo_root_id: None,
                generated_at: 0,
                granularity: "file".to_string(),
            },
            test_language: BTreeMap::new(),
            test_to_files: BTreeMap::new(),
            file_to_tests: BTreeMap::new(),
        };
        let mut bytes = wincode::serialize(&legacy).unwrap();
        bytes.push(0);

        assert!(deserialize_testmap(&bytes).is_err());
    }

    #[test]
    fn deserialize_v3_should_reject_sparse_row_count_mismatch() {
        let mut index = sample_sparse_index();
        index.sparse_coverage.pop();
        let bytes = encode_v3_unchecked(&index, TESTMAP_SCHEMA_VERSION, TESTMAP_SCHEMA_VERSION);

        let error = deserialize_testmap(&bytes).unwrap_err();

        assert!(
            error.to_string().contains("1 test rows for 2 tests"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn deserialize_v3_should_reject_out_of_range_file_index() {
        let mut index = sample_sparse_index();
        index.sparse_coverage[0].files[0].file_idx = index.file_index.len();
        let bytes = encode_v3_unchecked(&index, TESTMAP_SCHEMA_VERSION, TESTMAP_SCHEMA_VERSION);

        let error = deserialize_testmap(&bytes).unwrap_err();

        assert!(
            error.to_string().contains("references file index 3"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn deserialize_v3_should_reject_non_increasing_file_indexes() {
        let mut index = sample_sparse_index();
        index.sparse_coverage[0].files.swap(0, 1);
        let bytes = encode_v3_unchecked(&index, TESTMAP_SCHEMA_VERSION, TESTMAP_SCHEMA_VERSION);

        let error = deserialize_testmap(&bytes).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("file indexes are not strictly increasing"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn deserialize_v3_should_reject_empty_coverage_cell() {
        let mut index = sample_sparse_index();
        index.sparse_coverage[0].files[0].lines.clear();
        let bytes = encode_v3_unchecked(&index, TESTMAP_SCHEMA_VERSION, TESTMAP_SCHEMA_VERSION);

        let error = deserialize_testmap(&bytes).unwrap_err();

        assert!(
            error.to_string().contains("empty coverage cell"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn deserialize_v3_should_reject_malformed_bitmap_bytes() {
        let index = sample_sparse_index();
        let mut sparse_coverage = encode_sparse_coverage(&index.sparse_coverage).unwrap();
        sparse_coverage[0].files[0].lines = vec![0xff];
        let bytes = encode_v3_with_stored_rows(
            &index,
            &sparse_coverage,
            TESTMAP_SCHEMA_VERSION,
            TESTMAP_SCHEMA_VERSION,
        );

        let error = deserialize_testmap(&bytes).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Failed to deserialize testmap bitmap"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn deserialize_v3_should_reject_trailing_bitmap_bytes() {
        let index = sample_sparse_index();
        let mut sparse_coverage = encode_sparse_coverage(&index.sparse_coverage).unwrap();
        sparse_coverage[0].files[0].lines.push(0);
        let bytes = encode_v3_with_stored_rows(
            &index,
            &sparse_coverage,
            TESTMAP_SCHEMA_VERSION,
            TESTMAP_SCHEMA_VERSION,
        );

        let error = deserialize_testmap(&bytes).unwrap_err();

        assert!(
            error.to_string().contains("trailing bytes"),
            "unexpected error: {error}"
        );
    }
}
