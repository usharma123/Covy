use super::*;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ignore::WalkBuilder;
use suite_packet_core::CovyError;

pub(crate) struct RepoScanAccumulator {
    root: PathBuf,
    cache: RepoScanCache,
    cache_dirty: bool,
    seen: BTreeSet<String>,
    out: Vec<FileScan>,
}

impl RepoScanAccumulator {
    pub(crate) fn new(root: &Path, source_paths: &[String]) -> Self {
        Self {
            root: root.to_path_buf(),
            cache: load_scan_cache(root),
            cache_dirty: false,
            seen: source_paths.iter().cloned().collect(),
            out: Vec::new(),
        }
    }

    pub(crate) fn ingest(&mut self, rel: &str, metadata: &Metadata, bytes: &[u8]) {
        let size = metadata.len();
        let mtime_secs = metadata_mtime_secs(metadata);
        let mtime_unix_nanos = metadata_mtime_unix_nanos(metadata);
        let Ok(content) = std::str::from_utf8(bytes) else {
            self.cache_dirty |= self.cache.files.remove(rel).is_some();
            return;
        };
        let content_fingerprint = content_fingerprint(content);

        if let Some(entry) = self.cache.files.get_mut(rel) {
            if entry.size == size && entry.content_fingerprint == content_fingerprint {
                if entry.mtime_secs != mtime_secs || entry.mtime_unix_nanos != mtime_unix_nanos {
                    entry.mtime_secs = mtime_secs;
                    entry.mtime_unix_nanos = mtime_unix_nanos;
                    self.cache_dirty = true;
                }
                self.out.push(FileScan {
                    path: rel.to_string(),
                    size,
                    symbols: entry.symbols.clone(),
                    symbol_defs: entry.symbol_defs.clone(),
                    imports: entry.imports.clone(),
                    token_lines: entry.token_lines.clone(),
                    mtime_secs,
                });
                return;
            }
        }

        let (symbol_defs, imports, token_lines) = extract_index_metadata(rel, content);
        let symbols = symbol_defs
            .iter()
            .map(|symbol| (symbol.kind.clone(), symbol.name.clone()))
            .collect::<Vec<_>>();
        self.cache.files.insert(
            rel.to_string(),
            CacheEntry {
                size,
                mtime_secs,
                mtime_unix_nanos,
                content_fingerprint,
                symbols: symbols.clone(),
                symbol_defs: symbol_defs.clone(),
                imports: imports.clone(),
                token_lines: token_lines.clone(),
            },
        );
        self.cache_dirty = true;
        self.out.push(FileScan {
            path: rel.to_string(),
            size,
            symbols,
            symbol_defs,
            imports,
            token_lines,
            mtime_secs,
        });
    }

    pub(crate) fn finish(mut self) -> Vec<FileScan> {
        let original_cache_len = self.cache.files.len();
        self.cache.files.retain(|path, _| self.seen.contains(path));
        self.cache_dirty |= self.cache.files.len() != original_cache_len;
        if self.cache_dirty {
            write_scan_cache(&self.root, &self.cache);
        }
        self.out.sort_by(|left, right| left.path.cmp(&right.path));
        self.out
    }
}

pub(crate) fn scan_repo(root: &Path, include_tests: bool) -> Result<Vec<FileScan>, CovyError> {
    scan_repo_with_progress(root, include_tests, |_, _| {})
}

pub(crate) fn scan_repo_with_progress<F>(
    root: &Path,
    include_tests: bool,
    mut on_progress: F,
) -> Result<Vec<FileScan>, CovyError>
where
    F: FnMut(usize, usize),
{
    let source_paths = discover_source_paths(root, include_tests)?;
    let mut accumulator = RepoScanAccumulator::new(root, &source_paths);
    let total_files = source_paths.len();
    on_progress(0, total_files);

    for (idx, rel) in source_paths.iter().enumerate() {
        let path = root.join(rel);

        let metadata = match std::fs::metadata(&path) {
            Ok(value) => value,
            Err(_) => {
                on_progress(idx + 1, total_files);
                continue;
            }
        };
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => {
                on_progress(idx + 1, total_files);
                continue;
            }
        };
        accumulator.ingest(rel, &metadata, &bytes);
        on_progress(idx + 1, total_files);
    }
    Ok(accumulator.finish())
}

fn discover_source_paths(root: &Path, include_tests: bool) -> Result<Vec<String>, CovyError> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .parents(true)
        .ignore(true)
        .git_ignore(true);
    let root_owned = root.to_path_buf();
    builder.filter_entry(move |entry| {
        if entry.depth() == 0 {
            return true;
        }
        let rel = entry
            .path()
            .strip_prefix(&root_owned)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .replace('\\', "/");
        !is_generated_or_vendor_path(&rel)
    });

    let mut out = Vec::new();
    for entry in builder.build() {
        let entry = entry.map_err(|source| {
            CovyError::Other(format!(
                "failed to walk repository '{}': {source}",
                root.display()
            ))
        })?;

        let path = entry.path();
        if !path.is_file() || !is_source_file(path) {
            continue;
        }

        let relative = path.strip_prefix(root).unwrap_or(path);
        let Some(rel) = normalized_utf8_repository_path(relative) else {
            continue;
        };
        if !include_tests && is_test_path(&rel) {
            continue;
        }
        out.push(rel);
    }
    out.sort();
    Ok(out)
}

pub(crate) fn scan_cache_path(root: &Path) -> PathBuf {
    root.join(MAP_CACHE_DIR).join(MAP_CACHE_FILE)
}

pub(crate) fn load_scan_cache(root: &Path) -> RepoScanCache {
    let path = scan_cache_path(root);
    let raw = if let Ok(raw) = std::fs::read(&path) {
        raw
    } else {
        let legacy_path = root.join(MAP_CACHE_DIR).join(MAP_CACHE_FILE_LEGACY);
        let Ok(raw) = std::fs::read(legacy_path) else {
            return empty_cache();
        };
        raw
    };

    let cache = if let Ok(cache) = wincode::deserialize::<RepoScanCache>(&raw) {
        cache
    } else if let Ok(cache) = serde_json::from_slice::<RepoScanCache>(&raw) {
        cache
    } else {
        return empty_cache();
    };

    if cache.version != MAP_CACHE_VERSION {
        return empty_cache();
    }

    cache
}

pub(crate) fn write_scan_cache(root: &Path, cache: &RepoScanCache) {
    let path = scan_cache_path(root);
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }

    let Ok(encoded) = wincode::serialize(cache) else {
        return;
    };

    let _ = std::fs::write(path, encoded);
}

pub(crate) fn empty_cache() -> RepoScanCache {
    RepoScanCache {
        version: MAP_CACHE_VERSION,
        files: BTreeMap::new(),
    }
}

pub(crate) fn metadata_mtime_secs(metadata: &Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(crate) fn metadata_mtime_unix_nanos(metadata: &Metadata) -> Option<i128> {
    metadata.modified().ok().map(system_time_unix_nanos)
}

pub(crate) fn system_time_unix_nanos(time: SystemTime) -> i128 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i128::try_from(duration.as_nanos()).unwrap_or(i128::MAX),
        Err(error) => -i128::try_from(error.duration().as_nanos()).unwrap_or(i128::MAX),
    }
}

pub(crate) fn content_fingerprint(content: &str) -> String {
    suite_packet_core::canonical_hash_json(&content)
}

pub(crate) fn is_source_file(path: &Path) -> bool {
    detect_source_language(&path.to_string_lossy()).is_some()
}

pub(crate) fn normalized_utf8_repository_path(path: &Path) -> Option<String> {
    path.to_str().map(|path| path.replace('\\', "/"))
}

pub(crate) fn is_generated_or_vendor_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower
        .split('/')
        .any(|segment| segment == ".tmp" || segment == ".temp" || segment.starts_with(".tmp-"))
    {
        return true;
    }
    lower.starts_with(".git/")
        || lower.contains("/.git/")
        || lower.starts_with("target/")
        || lower.contains("/target/")
        || lower.starts_with("build/")
        || lower.contains("/build/")
        || lower.starts_with("dist/")
        || lower.contains("/dist/")
        || lower.starts_with("out/")
        || lower.contains("/out/")
        || lower.starts_with("coverage/")
        || lower.contains("/coverage/")
        || lower.starts_with("node_modules/")
        || lower.contains("/node_modules/")
        || lower.contains("/jacoco-resources/")
}

pub(crate) fn is_test_path(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_ascii_lowercase();
    lower.starts_with("tests/")
        || lower.starts_with("test/")
        || lower.contains("/tests/")
        || lower.contains("/test/")
        || lower.ends_with("_test.rs")
        || lower.ends_with("_test.py")
        || lower.ends_with("/test.rs")
        || lower == "test.rs"
}

pub(crate) fn extract_imports(content: &str) -> Vec<String> {
    let mut out = BTreeSet::<String>::new();

    for cap in import_re().captures_iter(content) {
        let target = cap.name("target").map(|m| m.as_str()).unwrap_or("").trim();
        if target.is_empty() {
            continue;
        }
        let matched_line = cap.get(0).map(|m| m.as_str()).unwrap_or("");
        let resolved = if matched_line.trim_start().starts_with("import static ") {
            resolve_java_import_reference(target, true)
        } else {
            normalize_import_reference(target)
        };
        if let Some(normalized) = resolved {
            out.insert(normalized);
        }
    }

    out.into_iter().collect()
}

pub(crate) fn extract_index_metadata(
    path: &str,
    content: &str,
) -> (
    Vec<IndexedSymbolDef>,
    Vec<String>,
    BTreeMap<String, Vec<usize>>,
) {
    let (symbol_defs, imports) = if let Some(language) = detect_source_language(path) {
        extract_metadata_ast_with_lines(language, content)
            .unwrap_or_else(|| extract_metadata_regex_with_lines(path, content))
    } else {
        extract_metadata_regex_with_lines(path, content)
    };
    let token_lines = extract_token_lines(content, &symbol_defs);
    (symbol_defs, imports, token_lines)
}

pub(crate) fn extract_metadata_regex_with_lines(
    _path: &str,
    content: &str,
) -> (Vec<IndexedSymbolDef>, Vec<String>) {
    let mut out = BTreeSet::<IndexedSymbolDef>::new();
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        for cap in symbol_re().captures_iter(line) {
            let kind = cap.name("kind").map(|m| m.as_str()).unwrap_or("");
            let name = cap.name("name").map(|m| m.as_str()).unwrap_or("");
            if !name.is_empty() {
                out.insert(IndexedSymbolDef {
                    kind: kind.to_string(),
                    name: name.to_string(),
                    line: line_no,
                });
            }
        }
        for cap in java_type_re().captures_iter(line) {
            let kind = cap
                .name("kind")
                .map(|m| m.as_str())
                .unwrap_or("class")
                .to_ascii_lowercase();
            let name = cap.name("name").map(|m| m.as_str()).unwrap_or("");
            if !name.is_empty() {
                out.insert(IndexedSymbolDef {
                    kind,
                    name: name.to_string(),
                    line: line_no,
                });
            }
        }
        for cap in java_method_re().captures_iter(line) {
            let name = cap.name("name").map(|m| m.as_str()).unwrap_or("").trim();
            if !name.is_empty() && !is_reserved_word(name) {
                out.insert(IndexedSymbolDef {
                    kind: "method".to_string(),
                    name: name.to_string(),
                    line: line_no,
                });
            }
        }
    }
    (out.into_iter().collect(), extract_imports(content))
}

pub(crate) fn extract_token_lines(
    content: &str,
    symbol_defs: &[IndexedSymbolDef],
) -> BTreeMap<String, Vec<usize>> {
    let mut lines_by_token = BTreeMap::<String, Vec<usize>>::new();
    let symbol_tokens = symbol_defs
        .iter()
        .map(|symbol| symbol.name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        for cap in identifier_re().captures_iter(line) {
            let token = cap
                .name("token")
                .map(|m| m.as_str())
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if token.len() < 3 || is_reserved_word(&token) {
                continue;
            }
            if !symbol_tokens.contains(&token) && token.len() < 4 {
                continue;
            }
            let entry = lines_by_token.entry(token).or_default();
            if entry.last().copied() == Some(line_no) || entry.len() >= 8 {
                continue;
            }
            entry.push(line_no);
        }
    }
    for symbol in symbol_defs {
        let key = symbol.name.to_ascii_lowercase();
        let entry = lines_by_token.entry(key).or_default();
        if !entry.contains(&symbol.line) {
            entry.insert(0, symbol.line);
            entry.truncate(8);
        }
    }
    lines_by_token
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn utf8_repository_path_normalization_is_lossless_or_rejected() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;

        let mut invalid_name = b"collision_".to_vec();
        invalid_name.push(0xff);
        invalid_name.extend_from_slice(b".rs");
        let invalid = Path::new("src").join(OsString::from_vec(invalid_name));

        assert_eq!(normalized_utf8_repository_path(&invalid), None);
        assert_eq!(
            normalized_utf8_repository_path(Path::new("src/collision_\u{fffd}.rs")).as_deref(),
            Some("src/collision_\u{fffd}.rs")
        );
    }

    // APFS rejects creation of the invalid-byte fixture with EPERM, while
    // Linux filesystems preserve the byte name. The pure Unix test above still
    // verifies the macOS eligibility boundary.
    #[cfg(target_os = "linux")]
    #[test]
    fn scan_skips_non_utf8_paths_even_when_the_lossy_name_exists() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;

        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();

        let mut invalid_name = b"collision_".to_vec();
        invalid_name.push(0xff);
        invalid_name.extend_from_slice(b".rs");
        std::fs::write(
            root.join("src").join(OsString::from_vec(invalid_name)),
            "pub fn non_utf8_filename_symbol() {}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/collision_\u{fffd}.rs"),
            "pub fn utf8_replacement_filename_symbol() {}\n",
        )
        .unwrap();

        let scans = scan_repo(root, true).unwrap();
        assert_eq!(scans.len(), 1);
        assert_eq!(scans[0].path, "src/collision_\u{fffd}.rs");
        assert!(scans[0]
            .symbols
            .iter()
            .any(|(_, name)| name == "utf8_replacement_filename_symbol"));
        assert!(scans[0]
            .symbols
            .iter()
            .all(|(_, name)| name != "non_utf8_filename_symbol"));
    }
}
