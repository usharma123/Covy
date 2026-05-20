use std::fs;
use std::path::Path;

pub fn write_repo_fixture(root: &Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        "pub struct AlphaUniqueToken;\npub fn alpha_unique_token() -> &'static str { \"AlphaUniqueToken\" }\n",
    )
    .unwrap();
}
