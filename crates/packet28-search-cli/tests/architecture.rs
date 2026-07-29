#[test]
fn binary_entrypoint_stays_a_thin_process_boundary() {
    let source = include_str!("../src/main.rs");
    let substantive_lines = source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("//"))
        .collect::<Vec<_>>();

    assert_eq!(
        substantive_lines,
        [
            "fn main() -> std::process::ExitCode {",
            "packet28_search_cli::main_entry()",
            "}",
        ]
    );
}
