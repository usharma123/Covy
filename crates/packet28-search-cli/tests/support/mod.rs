use assert_cmd::Command;
use std::fs;
use std::path::Path;
use std::process::Output;

pub fn cli() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("p28"))
}

pub fn write_fixture(root: &Path) {
    fs::create_dir_all(root.join("src/nested")).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        "pub struct Alpha;\npub fn alpha_service() {}\nconst ALPHA: &str = \"Alpha\";\n",
    )
    .unwrap();
    fs::write(
        root.join("src/nested/mod.rs"),
        "pub enum Beta { AlphaVariant }\nfn handle_value() { println!(\"beta\"); }\n",
    )
    .unwrap();
    for idx in 0..10 {
        fs::write(
            root.join("src").join(format!("filler_{idx}.rs")),
            format!("pub fn filler_{idx}() {{ println!(\"beta_{idx}\"); }}\n"),
        )
        .unwrap();
    }
}

pub fn output(mut command: Command) -> Output {
    command.output().expect("command output")
}

pub fn stdout_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

pub fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}
