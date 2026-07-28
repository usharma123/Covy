use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target_os.as_str() {
        "linux" => {
            compile_interpose("src/linux_interpose.c", "linux_interpose.o", &["-fPIC"]);
            println!("cargo:rustc-link-lib=dl");
            println!("cargo:rustc-link-lib=pthread");
        }
        "macos" => {
            compile_interpose("src/macos_interpose.c", "macos_interpose.o", &[]);
        }
        _ => {}
    }

    println!("cargo:rerun-if-changed=src/linux_interpose.c");
    println!("cargo:rerun-if-changed=src/macos_interpose.c");
}

fn compile_interpose(source: &str, object_name: &str, target_flags: &[&str]) {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    let object_path = out_dir.join(object_name);
    let mut command = Command::new(c_compiler());
    command
        .arg("-std=c11")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-Werror")
        .args(target_flags)
        .arg("-c")
        .arg(source)
        .arg("-o")
        .arg(&object_path);
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("failed to invoke C compiler for '{source}': {error}"));
    assert!(
        status.success(),
        "C compiler failed for interpose bridge '{source}'"
    );
    // The bridge defines process-global interposition symbols and references
    // Rust callbacks that exist only in the library. Linking it into unit or
    // integration-test executables would both interpose their own I/O and
    // leave the callbacks unresolved.
    println!("cargo:rustc-cdylib-link-arg={}", object_path.display());
}

fn c_compiler() -> OsString {
    let target = std::env::var("TARGET").unwrap_or_default();
    let target_underscored = target.replace('-', "_");
    [
        format!("CC_{target}"),
        format!("CC_{target_underscored}"),
        "TARGET_CC".to_string(),
        "CC".to_string(),
    ]
    .into_iter()
    .find_map(std::env::var_os)
    .unwrap_or_else(|| OsString::from(default_c_compiler(&target)))
}

fn default_c_compiler(target: &str) -> &str {
    if target.contains("msvc") {
        "cl"
    } else {
        "cc"
    }
}
