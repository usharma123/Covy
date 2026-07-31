use std::path::PathBuf;

use covy_core::diagnostics::{DiagnosticsData, DiagnosticsFormat};
use covy_core::CovyError;

fn fixture(rel: &str) -> PathBuf {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    workspace.join("tests").join("fixtures").join(rel)
}

#[test]
fn test_ingest_lcov() {
    let path = fixture("lcov/basic.info");
    let data = covy_ingest::ingest_path(&path).unwrap();

    assert_eq!(data.files.len(), 2);
    assert!(data.files.contains_key("src/main.rs"));
    assert!(data.files.contains_key("src/lib.rs"));

    let main = &data.files["src/main.rs"];
    assert_eq!(main.lines_instrumented.len(), 4);
    assert_eq!(main.lines_covered.len(), 3);
}

#[test]
fn test_ingest_cobertura() {
    let path = fixture("cobertura/basic.xml");
    let data = covy_ingest::ingest_path(&path).unwrap();

    assert_eq!(data.files.len(), 1);
    let fc = &data.files["main.py"];
    assert_eq!(fc.lines_instrumented.len(), 5);
    assert_eq!(fc.lines_covered.len(), 3);
}

#[test]
fn test_ingest_jacoco() {
    let path = fixture("jacoco/basic.xml");
    let data = covy_ingest::ingest_path(&path).unwrap();

    assert_eq!(data.files.len(), 2);
    assert!(data.files.contains_key("com/example/App.java"));
    assert!(data.files.contains_key("com/example/Util.java"));

    let app = &data.files["com/example/App.java"];
    assert_eq!(app.lines_instrumented.len(), 4);
    assert_eq!(app.lines_covered.len(), 3);
}

#[test]
fn test_ingest_gocov() {
    let path = fixture("gocov/basic.out");
    let data = covy_ingest::ingest_path(&path).unwrap();

    assert_eq!(data.files.len(), 2);
    assert!(data.files.contains_key("pkg/handler.go"));
    assert!(data.files.contains_key("main.go"));

    let handler = &data.files["pkg/handler.go"];
    assert_eq!(handler.lines_instrumented.len(), 10);
    assert_eq!(handler.lines_covered.len(), 6);
}

#[test]
fn test_format_detection_lcov() {
    use std::path::Path;
    let content = b"TN:test\nSF:src/main.rs\n";
    let format = covy_ingest::detect_format(Path::new("coverage.info"), content).unwrap();
    assert_eq!(format, covy_core::CoverageFormat::Lcov);
}

#[test]
fn test_format_detection_cobertura() {
    use std::path::Path;
    let content = b"<?xml version=\"1.0\" ?>\n<coverage version=\"5\">";
    let format = covy_ingest::detect_format(Path::new("coverage.xml"), content).unwrap();
    assert_eq!(format, covy_core::CoverageFormat::Cobertura);
}

#[test]
fn test_format_detection_jacoco() {
    use std::path::Path;
    let content = b"<?xml version=\"1.0\"?>\n<!DOCTYPE report PUBLIC";
    let format = covy_ingest::detect_format(Path::new("jacoco.xml"), content).unwrap();
    assert_eq!(format, covy_core::CoverageFormat::JaCoCo);
}

#[test]
fn test_format_detection_gocov() {
    use std::path::Path;
    let content = b"mode: set\n";
    let format = covy_ingest::detect_format(Path::new("coverage.out"), content).unwrap();
    assert_eq!(format, covy_core::CoverageFormat::GoCov);
}

#[test]
fn test_merge_coverage_data() {
    let path = fixture("lcov/basic.info");
    let mut data1 = covy_ingest::ingest_path(&path).unwrap();
    let data2 = covy_ingest::ingest_path(&path).unwrap();

    data1.merge(&data2);
    assert_eq!(data1.files.len(), 2);
}

#[test]
fn test_ingest_sarif_diagnostics() {
    let path = fixture("sarif/basic.sarif");
    let data = covy_ingest::ingest_diagnostics_path(&path).unwrap();

    assert_eq!(data.total_issues(), 5);
    assert!(data.issues_by_file.contains_key("src/main.rs"));
    assert!(data.issues_by_file.contains_key("src/lib.rs"));
}

#[test]
fn test_ingest_empty_sarif_diagnostics() {
    let path = fixture("sarif/empty.sarif");
    let data = covy_ingest::ingest_diagnostics_path(&path).unwrap();
    assert_eq!(data.total_issues(), 0);
}

#[test]
fn test_detect_diagnostics_format_sarif() {
    let content = br#"{\"$schema\":\"https://json.schemastore.org/sarif-2.1.0.json\"}"#;
    let format =
        covy_ingest::detect_diagnostics_format(std::path::Path::new("x.sarif"), content).unwrap();
    assert_eq!(format, covy_core::diagnostics::DiagnosticsFormat::Sarif);
}

fn diagnostics_result_signature(
    result: Result<DiagnosticsData, CovyError>,
) -> Result<serde_json::Value, String> {
    result
        .map(|mut diagnostics| {
            diagnostics.timestamp = 0;
            serde_json::to_value(diagnostics).unwrap()
        })
        .map_err(|error| error.to_string())
}

#[test]
fn diagnostics_path_and_reader_match_across_input_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let cases: &[(&str, &[u8])] = &[
        ("empty", b""),
        ("whitespace", b" \n\t"),
        ("minimal-object", br#"{}"#),
        ("empty-runs", br#"{"version":"2.1.0","runs":[]}"#),
        ("malformed", br#"{"version":"2.1.0","runs":["#),
        ("invalid-utf8", b"{\"runs\":[]}\xff"),
    ];

    for (name, content) in cases {
        let path = dir.path().join(format!("{name}.sarif"));
        std::fs::write(&path, content).unwrap();

        let from_path = diagnostics_result_signature(covy_ingest::ingest_diagnostics_path(&path));
        let from_reader = diagnostics_result_signature(covy_ingest::ingest_diagnostics_reader(
            std::io::Cursor::new(*content),
            DiagnosticsFormat::Sarif,
        ));

        assert_eq!(
            from_path, from_reader,
            "path and reader ingestion diverged for {name}"
        );
    }
}

#[test]
fn diagnostics_single_buffer_parser_matches_public_entry_points() {
    let path = fixture("sarif/basic.sarif");
    let content = std::fs::read(&path).unwrap();

    let from_path = diagnostics_result_signature(covy_ingest::ingest_diagnostics_path(&path));
    let from_reader = diagnostics_result_signature(covy_ingest::ingest_diagnostics_reader(
        std::io::Cursor::new(&content),
        DiagnosticsFormat::Sarif,
    ));
    let direct = diagnostics_result_signature(covy_ingest::sarif::parse_sarif(&content));

    assert_eq!(from_path, from_reader);
    assert_eq!(from_path, direct);
}
