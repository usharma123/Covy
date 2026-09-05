use std::fs;
use std::hint::black_box;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use packet28_reducer_core::SearchRequest;
use packet28_search_core::{
    guarded_fallback_reason, guarded_indexed_search, indexed_search,
    load_and_guarded_indexed_search, load_runtime, rebuild_full_index,
};

fn run_git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn benchmark_query_attestation(criterion: &mut Criterion) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        "pub fn unique_attestation_benchmark_needle() {}\n",
    )
    .unwrap();
    fs::write(root.join(".gitignore"), ".packet28/\n").unwrap();
    run_git(root, &["init", "--quiet"]);
    run_git(root, &["config", "user.name", "Packet28 Benchmark"]);
    run_git(root, &["config", "user.email", "packet28@example.invalid"]);
    run_git(root, &["add", "."]);
    run_git(
        root,
        &["commit", "--quiet", "--no-gpg-sign", "-m", "fixture"],
    );
    let runtime = rebuild_full_index(root, true).unwrap();
    let request = SearchRequest {
        query: "unique_attestation_benchmark_needle".to_string(),
        fixed_string: true,
        ..SearchRequest::default()
    };
    let mut group = criterion.benchmark_group("query_workspace_attestation");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(1));
    group.bench_function("combined_guard_and_search", |bencher| {
        bencher.iter(|| {
            black_box(guarded_indexed_search(root, &runtime, &request).unwrap());
        });
    });
    group.bench_function("split_guard_then_search_reference", |bencher| {
        bencher.iter(|| {
            assert_eq!(
                guarded_fallback_reason(root, &runtime, &request).unwrap(),
                None
            );
            black_box(indexed_search(root, &runtime, &request).unwrap());
        });
    });
    group.bench_function("from_disk_combined_guard_and_search", |bencher| {
        bencher.iter(|| {
            black_box(load_and_guarded_indexed_search(root, &request).unwrap());
        });
    });
    group.bench_function("from_disk_split_reference", |bencher| {
        bencher.iter(|| {
            let loaded = load_runtime(root).unwrap();
            assert_eq!(
                guarded_fallback_reason(root, &loaded, &request).unwrap(),
                None
            );
            black_box(indexed_search(root, &loaded, &request).unwrap());
        });
    });
    group.finish();
}

criterion_group!(benches, benchmark_query_attestation);
criterion_main!(benches);
