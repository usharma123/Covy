fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        use std::path::PathBuf;
        use std::process::Command;

        let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set"));
        let object_path = out_dir.join("macos_interpose.o");
        let status = Command::new("cc")
            .arg("-c")
            .arg("src/macos_interpose.c")
            .arg("-o")
            .arg(&object_path)
            .status()
            .expect("failed to invoke cc for macOS interpose shim");
        assert!(
            status.success(),
            "cc failed to compile macOS interpose shim"
        );
        println!("cargo:rustc-link-arg={}", object_path.display());
    }
    println!("cargo:rerun-if-changed=src/macos_interpose.c");
}
