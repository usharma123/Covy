use std::process::Command;

#[test]
fn binary_serve_reports_the_library_startup_error() {
    let directory = tempfile::tempdir().unwrap();
    let missing_root = directory.path().join("missing-workspace");
    let library_error = packet28d::serve(missing_root.clone()).unwrap_err();

    let output = Command::new(env!("CARGO_BIN_EXE_packet28d"))
        .args(["serve", "--root"])
        .arg(missing_root)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!("error: {library_error:#}\n")
    );
}
