use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::path::{Path, PathBuf};
use std::time::Duration;

const SCALED_LCOV_RECORDS: usize = 50_000;
const DIAGNOSTICS_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;

fn fixture_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests")
        .join("fixtures")
        .join(rel)
}

fn bench_lcov_ingest(c: &mut Criterion) {
    let content = std::fs::read(fixture_path("lcov/basic.info")).unwrap();
    let ingestor = covy_ingest::get_ingestor(covy_core::CoverageFormat::Lcov);

    c.bench_function("lcov_parse", |b| {
        b.iter(|| {
            let _ = ingestor.parse(black_box(&content));
        })
    });
}

fn scaled_lcov(records: usize) -> Vec<u8> {
    let mut content = String::with_capacity(records.saturating_mul(28));
    content.push_str("TN:allocation-benchmark\nSF:src/generated.rs\n");
    for line in 1..=records {
        content.push_str(&format!("DA:{line},{},checksum\n", line % 2));
    }
    content.push_str("end_of_record\n");
    content.into_bytes()
}

fn bench_lcov_ingest_scaled(c: &mut Criterion) {
    let content = scaled_lcov(SCALED_LCOV_RECORDS);
    let ingestor = covy_ingest::get_ingestor(covy_core::CoverageFormat::Lcov);
    let mut group = c.benchmark_group("lcov_scaled");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(SCALED_LCOV_RECORDS as u64));
    group.bench_with_input(
        BenchmarkId::new("public_parser", SCALED_LCOV_RECORDS),
        &content,
        |b, content| {
            b.iter(|| ingestor.parse(black_box(content)).unwrap());
        },
    );
    group.finish();
}

fn da_records(records: usize) -> Vec<String> {
    (1..=records)
        .map(|line| format!("{line},{},checksum", line % 2))
        .collect()
}

fn parse_da_fields_with_collect(records: &[String]) -> u64 {
    let mut checksum = 0u64;
    for record in records {
        let parts: Vec<&str> = record.splitn(3, ',').collect();
        if parts.len() >= 2 {
            if let Ok(line) = parts[0].parse::<u32>() {
                let count = parts[1].parse::<u64>().unwrap_or(0);
                checksum = checksum.wrapping_add(u64::from(line)).wrapping_add(count);
            }
        }
    }
    checksum
}

fn parse_da_fields_with_iterators(records: &[String]) -> u64 {
    let mut checksum = 0u64;
    for record in records {
        let mut parts = record.splitn(3, ',');
        if let (Some(line), Some(count)) = (parts.next(), parts.next()) {
            if let Ok(line) = line.parse::<u32>() {
                let count = count.parse::<u64>().unwrap_or(0);
                checksum = checksum.wrapping_add(u64::from(line)).wrapping_add(count);
            }
        }
    }
    checksum
}

fn bench_lcov_record_field_strategies(c: &mut Criterion) {
    let records = da_records(SCALED_LCOV_RECORDS);
    let mut group = c.benchmark_group("lcov_da_fields");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Elements(SCALED_LCOV_RECORDS as u64));
    group.bench_function("collect_vec", |b| {
        b.iter(|| black_box(parse_da_fields_with_collect(black_box(&records))));
    });
    group.bench_function("iterator_fields", |b| {
        b.iter(|| black_box(parse_da_fields_with_iterators(black_box(&records))));
    });
    group.finish();
}

fn scaled_sarif(payload_bytes: usize) -> Vec<u8> {
    let mut content = String::with_capacity(payload_bytes.saturating_add(128));
    content.push_str(r#"{"version":"2.1.0","runs":[],"benchmarkPadding":""#);
    content.extend(std::iter::repeat_n('a', payload_bytes));
    content.push_str("\"}");
    content.into_bytes()
}

fn ingest_diagnostics_single_buffer(
    path: &Path,
) -> Result<covy_core::diagnostics::DiagnosticsData, covy_core::CovyError> {
    let content = std::fs::read(path)?;
    let format = covy_ingest::detect_diagnostics_format(path, &content)?;
    match format {
        covy_core::diagnostics::DiagnosticsFormat::Sarif => {
            covy_ingest::sarif::parse_sarif(&content)
        }
    }
}

fn bench_diagnostics_whole_file_copy(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scaled.sarif");
    let content = scaled_sarif(DIAGNOSTICS_PAYLOAD_BYTES);
    std::fs::write(&path, &content).unwrap();

    let mut group = c.benchmark_group("diagnostics_whole_file");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));
    group.throughput(Throughput::Bytes(content.len() as u64));
    group.bench_function("auto_path_api", |b| {
        b.iter(|| covy_ingest::ingest_diagnostics_path(black_box(&path)).unwrap());
    });
    group.bench_function("single_buffer_reference", |b| {
        b.iter(|| ingest_diagnostics_single_buffer(black_box(&path)).unwrap());
    });
    group.finish();
}

fn bench_cobertura_ingest(c: &mut Criterion) {
    let content = std::fs::read(fixture_path("cobertura/basic.xml")).unwrap();
    let ingestor = covy_ingest::get_ingestor(covy_core::CoverageFormat::Cobertura);

    c.bench_function("cobertura_parse", |b| {
        b.iter(|| {
            let _ = ingestor.parse(black_box(&content));
        })
    });
}

fn bench_jacoco_ingest(c: &mut Criterion) {
    let content = std::fs::read(fixture_path("jacoco/basic.xml")).unwrap();
    let ingestor = covy_ingest::get_ingestor(covy_core::CoverageFormat::JaCoCo);

    c.bench_function("jacoco_parse", |b| {
        b.iter(|| {
            let _ = ingestor.parse(black_box(&content));
        })
    });
}

fn bench_gocov_ingest(c: &mut Criterion) {
    let content = std::fs::read(fixture_path("gocov/basic.out")).unwrap();
    let ingestor = covy_ingest::get_ingestor(covy_core::CoverageFormat::GoCov);

    c.bench_function("gocov_parse", |b| {
        b.iter(|| {
            let _ = ingestor.parse(black_box(&content));
        })
    });
}

criterion_group!(
    benches,
    bench_lcov_ingest,
    bench_lcov_ingest_scaled,
    bench_lcov_record_field_strategies,
    bench_diagnostics_whole_file_copy,
    bench_cobertura_ingest,
    bench_jacoco_ingest,
    bench_gocov_ingest
);
criterion_main!(benches);
