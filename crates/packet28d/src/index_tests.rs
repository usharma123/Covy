use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Barrier;
use std::time::{Duration, Instant};

use super::*;
use crate::tests::support::{daemon_test_root, daemon_test_state};

struct IndexFixture {
    state: Arc<Mutex<DaemonState>>,
    root: PathBuf,
}

impl IndexFixture {
    fn new(files: &[(&str, &str)]) -> Self {
        let state = daemon_test_state();
        let root = daemon_test_root(&state);
        for (path, contents) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().expect("fixture parent")).expect("create parent");
            fs::write(path, contents).expect("write fixture");
        }
        {
            let mut guard = state.lock().expect("state");
            guard.interactive_index.manifest = default_index_manifest(&root);
            guard
                .interactive_index
                .manifest
                .status
                .transition_to(DaemonIndexState::Queued)
                .expect("queue rebuild");
        }
        perform_full_index_rebuild(&state).expect("full rebuild");
        Self { state, root }
    }

    fn generated(file_count: usize) -> Self {
        let state = daemon_test_state();
        let root = daemon_test_root(&state);
        for idx in 0..file_count {
            write_generated_source(&root, idx, 0);
        }
        {
            let mut guard = state.lock().expect("state");
            guard.interactive_index.manifest = default_index_manifest(&root);
            guard
                .interactive_index
                .manifest
                .status
                .transition_to(DaemonIndexState::Queued)
                .expect("queue rebuild");
        }
        perform_full_index_rebuild(&state).expect("full rebuild");
        Self { state, root }
    }

    fn repo_runtime(&self) -> mapy_core::RepoIndexRuntime {
        self.state
            .lock()
            .expect("state")
            .interactive_index
            .repo_runtime
            .clone()
            .expect("repo runtime")
    }

    fn regex_runtime(&self) -> packet28_search_core::RegexIndexRuntime {
        self.state
            .lock()
            .expect("state")
            .interactive_index
            .regex_runtime
            .clone()
            .expect("regex runtime")
    }

    fn update(&self, paths: &[&str]) {
        let paths = paths
            .iter()
            .map(|path| (*path).to_string())
            .collect::<Vec<_>>();
        perform_incremental_index_update(&self.state, &paths).expect("incremental update");
    }
}

impl Drop for IndexFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn daemon_rebuild_and_one_file_update_preserve_parity_without_snapshot_rewrite() {
    let fixture = IndexFixture::new(&[
        ("src/a.rs", "pub fn alpha() -> usize { 1 }\n"),
        ("src/b.rs", "pub fn beta() -> usize { 2 }\n"),
    ]);
    let original_repo = fixture.repo_runtime();
    let original_regex = fixture.regex_runtime();
    let original_generation = fixture
        .state
        .lock()
        .expect("state")
        .interactive_index
        .manifest
        .generation;

    fs::write(
        fixture.root.join("src/a.rs"),
        "pub fn replacement() -> usize { 3 }\n",
    )
    .expect("replace source");
    fixture.update(&["src/a.rs"]);

    let updated_repo = fixture.repo_runtime();
    let updated_regex = fixture.regex_runtime();
    let guard = fixture.state.lock().expect("state");
    assert!(guard.interactive_index.repo_is_current());
    assert!(guard.interactive_index.regex_is_current());
    assert_eq!(
        guard.interactive_index.manifest.generation,
        original_generation + 1
    );
    assert_eq!(guard.interactive_index.manifest.total_files, 2);
    assert_eq!(guard.interactive_index.manifest.indexed_files, 2);
    drop(guard);

    assert!(original_repo.shares_base_with(&updated_repo));
    assert!(original_regex.shares_base_with(&updated_regex));
    assert!(original_repo
        .file("src/a.rs")
        .is_some_and(|entry| entry.symbols.iter().any(|symbol| symbol.name == "alpha")));
    assert!(updated_repo.file("src/a.rs").is_some_and(|entry| entry
        .symbols
        .iter()
        .any(|symbol| symbol.name == "replacement")));
    assert_eq!(
        updated_repo.materialize_snapshot(),
        Some(mapy_core::build_repo_index(&fixture.root, true).expect("parity snapshot"))
    );
    assert_search_parity(&fixture.root, &updated_regex, "replacement");
    assert!(!index_snapshot_path(&fixture.root).exists());
}

#[test]
fn daemon_delete_and_restart_reload_tombstones() {
    let fixture = IndexFixture::new(&[
        ("src/a.rs", "pub fn alpha() -> usize { 1 }\n"),
        ("src/b.rs", "pub fn beta() -> usize { 2 }\n"),
    ]);
    fs::remove_file(fixture.root.join("src/b.rs")).expect("delete source");

    fixture.update(&["src/b.rs"]);

    let repo_runtime = fixture.repo_runtime();
    let regex_runtime = fixture.regex_runtime();
    assert!(repo_runtime.file("src/b.rs").is_none());
    assert_eq!(repo_runtime.manifest.total_files, 1);
    assert_search_parity(&fixture.root, &regex_runtime, "beta");

    let restarted =
        load_index_runtime_files(&fixture.root, load_index_manifest_file(&fixture.root));
    assert!(
        !restarted.needs_rebuild(),
        "restart unexpectedly requested rebuild: daemon={:?}, repo={:?}, regex={:?}",
        restarted.manifest,
        restarted
            .repo_runtime
            .as_ref()
            .map(|runtime| &runtime.manifest),
        restarted
            .regex_runtime
            .as_ref()
            .map(|runtime| &runtime.manifest)
    );
    assert_eq!(
        restarted
            .repo_runtime
            .as_ref()
            .expect("reloaded repo")
            .manifest
            .generation,
        repo_runtime.manifest.generation
    );
    assert!(restarted
        .repo_runtime
        .as_ref()
        .expect("reloaded repo")
        .file("src/b.rs")
        .is_none());
    assert_search_parity(
        &fixture.root,
        restarted.regex_runtime.as_ref().expect("reloaded regex"),
        "beta",
    );
}

#[test]
fn daemon_restart_recovers_corruption_and_forces_a_clean_rebuild() {
    let fixture = IndexFixture::new(&[
        ("src/a.rs", "pub fn alpha() -> usize { 1 }\n"),
        ("src/b.rs", "pub fn beta() -> usize { 2 }\n"),
    ]);
    fs::write(
        fixture.root.join("src/a.rs"),
        "pub fn replacement() -> usize { 3 }\n",
    )
    .expect("replace source");
    fixture.update(&["src/a.rs"]);
    let corrupt_generation = fixture.repo_runtime().manifest.generation;
    let record_path = fixture
        .root
        .join(".packet28/index/mapy-v1")
        .join(format!("generation-{corrupt_generation:020}.json"));
    fs::write(&record_path, b"{").expect("corrupt generation record");

    let restarted =
        load_index_runtime_files(&fixture.root, load_index_manifest_file(&fixture.root));
    assert_ne!(
        restarted.manifest.status,
        DaemonIndexState::Missing,
        "persisted daemon manifest was not reloadable"
    );
    let recovered = restarted.repo_runtime.as_ref().expect("recovered repo");
    assert!(recovered.is_loaded());
    assert_eq!(
        recovered.manifest.recovered_from_generation,
        Some(corrupt_generation)
    );
    assert!(restarted.needs_rebuild());

    {
        let mut guard = fixture.state.lock().expect("state");
        guard.interactive_index = restarted;
    }
    fixture.update(&["src/a.rs"]);

    let repaired = fixture.state.lock().expect("state");
    assert!(repaired.interactive_index.repo_is_current());
    assert!(repaired.interactive_index.regex_is_current());
    assert!(!repaired.interactive_index.needs_rebuild());
    assert!(repaired
        .interactive_index
        .repo_runtime
        .as_ref()
        .expect("repaired repo")
        .file("src/a.rs")
        .is_some_and(|entry| entry
            .symbols
            .iter()
            .any(|symbol| symbol.name == "replacement")));
}

#[test]
fn daemon_generation_swap_keeps_concurrent_old_readers_alive() {
    let fixture = IndexFixture::new(&[
        ("src/a.rs", "pub fn alpha() -> usize { 1 }\n"),
        ("src/b.rs", "pub fn beta() -> usize { 2 }\n"),
    ]);
    let original = fixture.repo_runtime();
    fs::write(
        fixture.root.join("src/a.rs"),
        "pub fn replacement() -> usize { 3 }\n",
    )
    .expect("replace source");
    let barrier = Arc::new(Barrier::new(9));

    std::thread::scope(|scope| {
        let readers = (0..8)
            .map(|_| {
                let runtime = original.clone();
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    barrier.wait();
                    for _ in 0..256 {
                        assert!(runtime.file("src/a.rs").is_some_and(|entry| entry
                            .symbols
                            .iter()
                            .any(|symbol| symbol.name == "alpha")));
                    }
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        fixture.update(&["src/a.rs"]);
        for reader in readers {
            reader.join().expect("reader");
        }
    });

    let updated = fixture.repo_runtime();
    assert!(original.shares_base_with(&updated));
    assert!(updated.file("src/a.rs").is_some_and(|entry| entry
        .symbols
        .iter()
        .any(|symbol| symbol.name == "replacement")));
}

#[test]
fn daemon_clear_removes_generations_and_preserves_owned_readers() {
    let fixture = IndexFixture::new(&[
        ("src/a.rs", "pub fn alpha() -> usize { 1 }\n"),
        ("src/b.rs", "pub fn beta() -> usize { 2 }\n"),
    ]);
    let reader = fixture.repo_runtime();
    fs::write(index_snapshot_path(&fixture.root), b"legacy snapshot").expect("legacy artifact");

    perform_index_clear(&fixture.state).expect("clear");

    let guard = fixture.state.lock().expect("state");
    assert!(guard.interactive_index.repo_runtime.is_none());
    assert!(guard.interactive_index.regex_runtime.is_none());
    drop(guard);
    assert!(!fixture.root.join(".packet28/index/mapy-v1").exists());
    assert!(!fixture.root.join(".packet28/index/regex-v1").exists());
    assert!(!index_snapshot_path(&fixture.root).exists());
    assert!(reader.file("src/a.rs").is_some());
}

#[test]
fn daemon_incremental_publication_benchmark() {
    const FILE_COUNT: usize = 256;
    const UPDATES: usize = 5;

    let fixture = IndexFixture::generated(FILE_COUNT);
    let original = fixture.repo_runtime();
    let initial_tree = snapshot_tree(&fixture.root.join(".packet28/index"));
    let initial_generation_bytes = tree_bytes(&initial_tree);
    let mut samples = Vec::with_capacity(UPDATES);
    let mut publication_bytes = 0;

    for revision in 1..=UPDATES {
        write_generated_source(&fixture.root, 0, revision);
        let before = snapshot_tree(&fixture.root.join(".packet28/index"));
        let started = Instant::now();
        fixture.update(&["src/file_000.rs"]);
        samples.push(started.elapsed());
        let after = snapshot_tree(&fixture.root.join(".packet28/index"));
        publication_bytes = changed_file_bytes(&before, &after);
    }

    let median = median(samples);
    let updated = fixture.repo_runtime();
    println!(
        "daemon_incremental_median_us={} daemon_incremental_publication_bytes={} daemon_initial_generation_bytes={} legacy_snapshot_written={}",
        median.as_micros(),
        publication_bytes,
        initial_generation_bytes,
        index_snapshot_path(&fixture.root).exists()
    );
    assert!(original.shares_base_with(&updated));
    assert!(
        publication_bytes < initial_generation_bytes,
        "incremental publication {publication_bytes} should be smaller than initial generation {initial_generation_bytes}"
    );
    assert!(!index_snapshot_path(&fixture.root).exists());
}

fn assert_search_parity(
    root: &Path,
    runtime: &packet28_search_core::RegexIndexRuntime,
    query: &str,
) {
    let request = packet28_reducer_core::SearchRequest {
        query: query.to_string(),
        fixed_string: true,
        ..packet28_reducer_core::SearchRequest::default()
    };
    let indexed = packet28_search_core::indexed_search(root, runtime, &request).expect("indexed");
    let reducer = packet28_reducer_core::search(root, &request).expect("reducer");
    assert_eq!(indexed.match_count, reducer.match_count);
    assert_eq!(indexed.paths, reducer.paths);
    assert_eq!(indexed.regions, reducer.regions);
}

fn write_generated_source(root: &Path, idx: usize, revision: usize) {
    let mut body = format!(
        "pub fn file_{idx:03}_revision_{revision}(value: usize) -> usize {{\n    value + {revision}\n}}\n"
    );
    for line in 0..32 {
        body.push_str(&format!(
            "pub const FILE_{idx:03}_LINE_{line:02}: &str = \"packet28 daemon incremental benchmark\";\n"
        ));
    }
    let path = root.join(format!("src/file_{idx:03}.rs"));
    fs::create_dir_all(path.parent().expect("source parent")).expect("create source parent");
    fs::write(path, body).expect("write generated source");
}

fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).expect("read index directory") {
            let entry = entry.expect("index entry");
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, files);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .expect("relative artifact")
                    .to_path_buf();
                files.insert(relative, fs::read(path).expect("read artifact"));
            }
        }
    }

    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

fn changed_file_bytes(
    before: &BTreeMap<PathBuf, Vec<u8>>,
    after: &BTreeMap<PathBuf, Vec<u8>>,
) -> u64 {
    after
        .iter()
        .filter(|(path, bytes)| before.get(*path) != Some(*bytes))
        .map(|(_, bytes)| bytes.len() as u64)
        .sum()
}

fn tree_bytes(tree: &BTreeMap<PathBuf, Vec<u8>>) -> u64 {
    tree.values().map(|bytes| bytes.len() as u64).sum()
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}
