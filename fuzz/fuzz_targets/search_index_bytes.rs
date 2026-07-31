#![no_main]

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;

use libfuzzer_sys::fuzz_target;
use packet28_search_core::{load_runtime, rebuild_full_index};
use tempfile::TempDir;

const MAX_FUZZ_INPUT_BYTES: usize = 64 * 1024;
const INDEX_RELATIVE_DIR: &str = ".packet28/index/regex-v1";
const INDEX_FILES: [&str; 8] = [
    "manifest.json",
    "base.lookup.dat",
    "base.postings.dat",
    "docs.dat",
    "overlay.lookup.dat",
    "overlay.postings.dat",
    "overlay.docs.dat",
    "overlay.state.json",
];
const MUTATED_FILES: [&str; 2] = ["base.lookup.dat", "base.postings.dat"];

thread_local! {
    static FIXTURE: RefCell<IndexFixture> = RefCell::new(IndexFixture::new());
}

struct IndexFixture {
    _temp: TempDir,
    root: PathBuf,
    index_dir: PathBuf,
    baseline: Vec<Vec<u8>>,
}

impl IndexFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("create fuzz index directory");
        let root = temp.path().to_path_buf();
        let source_dir = root.join("src");
        fs::create_dir_all(&source_dir).expect("create fuzz source directory");
        fs::write(
            source_dir.join("lib.rs"),
            "pub struct Seed;\npub fn seed_value() -> usize { 28 }\n",
        )
        .expect("write fuzz source seed");

        let runtime = rebuild_full_index(&root, true).expect("build fuzz index seed");
        assert!(runtime.is_loaded(), "fuzz seed index must load");
        drop(runtime);

        let index_dir = root.join(INDEX_RELATIVE_DIR);
        let baseline = INDEX_FILES
            .iter()
            .map(|name| fs::read(index_dir.join(name)).expect("read fuzz index seed"))
            .collect();

        Self {
            _temp: temp,
            root,
            index_dir,
            baseline,
        }
    }

    fn exercise(&mut self, input: &[u8]) {
        let Some((&control, payload)) = input.split_first() else {
            return;
        };
        let payload = if matches!(control, b'D' | b'E') && payload.iter().all(|byte| *byte == b'\n')
        {
            &[][..]
        } else {
            payload
        };

        self.restore();
        let file_index = usize::from(control & 1);
        let baseline_index = file_index + 1;
        let candidate = mutate_bytes(&self.baseline[baseline_index], payload, (control >> 1) & 3);
        fs::write(self.index_dir.join(MUTATED_FILES[file_index]), candidate)
            .expect("write fuzzed index bytes");

        drop(load_runtime(&self.root));
    }

    fn restore(&self) {
        for (name, bytes) in INDEX_FILES.iter().zip(&self.baseline) {
            fs::write(self.index_dir.join(name), bytes).expect("restore fuzz index seed");
        }
    }
}

fn mutate_bytes(baseline: &[u8], payload: &[u8], strategy: u8) -> Vec<u8> {
    match strategy {
        0 => payload.to_vec(),
        1 => {
            let mut candidate = Vec::with_capacity(baseline.len() + payload.len());
            candidate.extend_from_slice(baseline);
            candidate.extend_from_slice(payload);
            candidate
        }
        2 => xor_bytes(baseline, payload),
        _ => {
            let keep = payload.len() % (baseline.len() + 1);
            baseline[..keep].to_vec()
        }
    }
}

fn xor_bytes(baseline: &[u8], payload: &[u8]) -> Vec<u8> {
    if baseline.is_empty() {
        return payload.to_vec();
    }
    let mut candidate = baseline.to_vec();
    let candidate_len = candidate.len();
    for (index, byte) in payload.iter().enumerate() {
        candidate[index % candidate_len] ^= byte;
    }
    candidate
}

fuzz_target!(|input: &[u8]| {
    if input.len() > MAX_FUZZ_INPUT_BYTES {
        return;
    }
    FIXTURE.with(|fixture| fixture.borrow_mut().exercise(input));
});
