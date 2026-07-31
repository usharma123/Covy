use std::path::Path;

pub fn ensure_packet28d_built() {
    crate::process_harness::ensure_packet28d_built();
}

fn git(root: &Path, args: &[&str]) {
    crate::process_harness::run_git(root, args);
}

pub fn init_repo(root: &Path) {
    git(root, &["init"]);
}
