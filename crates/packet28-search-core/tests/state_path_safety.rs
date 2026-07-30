use std::fs;

use packet28_search_core::{load_runtime, rebuild_full_index};
use packet28_state_fs::StateDir;

const MAX_REGEX_METADATA_BYTES: u64 = 8 * 1024 * 1024;
const MAX_REGEX_MMAP_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[cfg(unix)]
#[test]
fn rebuild_rejects_a_symlinked_state_parent_without_touching_the_victim() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let victim = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "pub struct Visible;\n").unwrap();
    let sentinel = victim.path().join("sentinel");
    fs::write(&sentinel, b"outside-must-survive").unwrap();
    symlink(victim.path(), dir.path().join(".packet28")).unwrap();

    let error = rebuild_full_index(dir.path(), true).unwrap_err();

    assert!(error.to_string().contains("regex writer lock"));
    assert_eq!(fs::read(&sentinel).unwrap(), b"outside-must-survive");
    assert!(!victim.path().join("index").exists());
}

#[cfg(unix)]
#[test]
fn retained_search_state_rejects_an_ancestor_swap_without_touching_the_victim() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let victim = tempfile::tempdir().unwrap();
    let state = StateDir::open(dir.path(), &[".packet28", "index", "regex-v1"], true).unwrap();
    let held = dir.path().join("held-packet28");
    let sentinel = victim.path().join("sentinel");
    fs::write(&sentinel, b"outside-must-survive").unwrap();
    fs::rename(dir.path().join(".packet28"), &held).unwrap();
    symlink(victim.path(), dir.path().join(".packet28")).unwrap();

    let error = state
        .write_atomic("manifest.json", b"replacement")
        .unwrap_err();

    assert!(matches!(
        error.kind(),
        std::io::ErrorKind::NotADirectory | std::io::ErrorKind::PermissionDenied
    ));
    assert_eq!(fs::read(&sentinel).unwrap(), b"outside-must-survive");
    assert!(!victim.path().join("manifest.json").exists());
    assert!(!held.join("index/regex-v1/manifest.json").exists());
}

#[test]
fn loader_rejects_oversized_sparse_metadata_and_mmaps() {
    let metadata_root = tempfile::tempdir().unwrap();
    let metadata_state = StateDir::open(
        metadata_root.path(),
        &[".packet28", "index", "regex-v1"],
        true,
    )
    .unwrap();
    fs::File::create(metadata_state.path().join("manifest.json"))
        .unwrap()
        .set_len(MAX_REGEX_METADATA_BYTES + 1)
        .unwrap();

    let metadata_runtime = load_runtime(metadata_root.path()).unwrap();

    assert!(!metadata_runtime.is_loaded());
    assert!(metadata_runtime
        .manifest
        .last_error
        .as_deref()
        .is_some_and(|error| error.contains("exceeds")));

    let mmap_root = tempfile::tempdir().unwrap();
    fs::create_dir_all(mmap_root.path().join("src")).unwrap();
    fs::write(mmap_root.path().join("src/lib.rs"), "pub struct Visible;\n").unwrap();
    let built = rebuild_full_index(mmap_root.path(), true).unwrap();
    let state =
        StateDir::open(mmap_root.path(), &[".packet28", "index", "regex-v1"], false).unwrap();
    let record: serde_json::Value = serde_json::from_slice(
        &fs::read(
            state
                .path()
                .join(format!("generation-{:020}.json", built.manifest.generation)),
        )
        .unwrap(),
    )
    .unwrap();
    let lookup = record["base"]["lookup"].as_str().unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(state.path().join(lookup))
        .unwrap()
        .set_len(MAX_REGEX_MMAP_BYTES + 1)
        .unwrap();

    let mmap_runtime = load_runtime(mmap_root.path()).unwrap();

    assert!(!mmap_runtime.is_loaded());
    assert!(mmap_runtime
        .manifest
        .last_error
        .as_deref()
        .is_some_and(|error| error.contains("exceeds")));
}
