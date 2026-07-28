use std::path::{Path, PathBuf};
use std::process::Command;

use roaring::RoaringBitmap;

pub use crate::error::DiffyError;
use crate::error::Result;
use crate::model::{DiffStatus, FileDiff};

const DIFF_CACHE_DIR: &str = ".covy/state/diff-cache";

/// Compute changed files and line numbers between two Git references.
///
/// The on-disk diff cache is best-effort: cache read or write failures do not
/// prevent a fresh Git diff from being returned.
///
/// # Errors
///
/// Returns [`DiffyError::GitNotFound`] when Git cannot be located,
/// [`DiffyError::GitSpawn`] when Git cannot be started, or
/// [`DiffyError::GitCommandFailed`] when reference resolution or diff execution
/// exits unsuccessfully.
pub fn git_diff(base: &str, head: &str) -> Result<Vec<FileDiff>> {
    let (base_hash, head_hash) = resolve_refs(base, head)?;
    let cache_path = diff_cache_path(&base_hash, &head_hash);

    if let Some(cached) = load_cached_diff(&cache_path) {
        return parse_diff_output(&cached);
    }

    let stdout = run_git_diff(base, head)?;
    let _ = save_cached_diff(&cache_path, &stdout);
    parse_diff_output(&stdout)
}

fn run_git_diff(base: &str, head: &str) -> Result<String> {
    let output = Command::new("git")
        .args([
            "diff",
            "--unified=0",
            "--no-color",
            "--no-ext-diff",
            "--diff-filter=ACMR",
            &format!("{base}..{head}"),
        ])
        .output()
        .map_err(|source| git_spawn_error("diff", source))?;

    if !output.status.success() {
        return Err(DiffyError::GitCommandFailed {
            operation: "diff",
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr)
                .trim_end()
                .to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn resolve_refs(base: &str, head: &str) -> Result<(String, String)> {
    let output = Command::new("git")
        .args(["rev-parse", base, head])
        .output()
        .map_err(|source| git_spawn_error("rev-parse", source))?;

    if !output.status.success() {
        return Err(DiffyError::GitCommandFailed {
            operation: "rev-parse",
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr)
                .trim_end()
                .to_string(),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let base_hash = lines.next().unwrap_or_default().trim().to_string();
    let head_hash = lines.next().unwrap_or_default().trim().to_string();

    if base_hash.is_empty() || head_hash.is_empty() {
        return Err(DiffyError::EmptyGitRefHash);
    }

    Ok((base_hash, head_hash))
}

fn git_spawn_error(operation: &'static str, source: std::io::Error) -> DiffyError {
    if source.kind() == std::io::ErrorKind::NotFound {
        DiffyError::GitNotFound { operation, source }
    } else {
        DiffyError::GitSpawn { operation, source }
    }
}

fn diff_cache_key(base_hash: &str, head_hash: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(base_hash.as_bytes());
    hasher.update(head_hash.as_bytes());
    hasher.finalize().to_hex().to_string()
}

fn diff_cache_path(base_hash: &str, head_hash: &str) -> PathBuf {
    Path::new(DIFF_CACHE_DIR).join(format!("{}.diff", diff_cache_key(base_hash, head_hash)))
}

fn load_cached_diff(path: &Path) -> Option<String> {
    if !path.exists() {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn save_cached_diff(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

/// Parse the raw output of `git diff --unified=0`.
///
/// # Errors
///
/// The current tolerant parser does not reject malformed lines. The typed
/// result leaves room for future validation without changing the API shape.
pub fn parse_diff_output(diff_text: &str) -> Result<Vec<FileDiff>> {
    let mut diffs = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_old_path: Option<String> = None;
    let mut current_status = DiffStatus::Modified;
    let mut current_lines = RoaringBitmap::new();

    for line in diff_text.lines() {
        if line.starts_with("diff --git") {
            // Flush previous file
            if let Some(path) = current_path.take() {
                diffs.push(FileDiff {
                    path,
                    old_path: current_old_path.take(),
                    status: current_status,
                    changed_lines: std::mem::take(&mut current_lines),
                });
            }
            current_status = DiffStatus::Modified;
            current_old_path = None;
        } else if let Some(stripped) = line.strip_prefix("+++ b/") {
            current_path = Some(stripped.to_string());
        } else if line.starts_with("+++ /dev/null") {
            // File was deleted — we skip deleted files (filtered by --diff-filter)
        } else if line.starts_with("new file") {
            current_status = DiffStatus::Added;
        } else if let Some(stripped) = line.strip_prefix("rename from ") {
            current_old_path = Some(stripped.to_string());
            current_status = DiffStatus::Renamed;
        } else if line.starts_with("@@") {
            // Parse @@ -old,count +new,count @@ ...
            if let Some(new_range) = parse_hunk_header(line) {
                for line_no in new_range.0..=new_range.1 {
                    current_lines.insert(line_no);
                }
            }
        }
    }

    // Flush last file
    if let Some(path) = current_path {
        diffs.push(FileDiff {
            path,
            old_path: current_old_path,
            status: current_status,
            changed_lines: current_lines,
        });
    }

    Ok(diffs)
}

/// Parse a hunk header like `@@ -10,3 +20,5 @@` and return (start, end) for the new side.
fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
    // Find the +N,M or +N part
    let plus_idx = line.find('+').unwrap_or(0);
    let rest = &line[plus_idx + 1..];
    let end = rest.find(' ').unwrap_or(rest.len());
    let range_str = &rest[..end];

    if let Some(comma) = range_str.find(',') {
        let start: u32 = range_str[..comma].parse().ok()?;
        let count: u32 = range_str[comma + 1..].parse().ok()?;
        if count == 0 {
            return None; // Pure deletion in old, no new lines
        }
        Some((start, start + count - 1))
    } else {
        let start: u32 = range_str.parse().ok()?;
        Some((start, start))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hunk_single_line() {
        assert_eq!(parse_hunk_header("@@ -1 +1 @@"), Some((1, 1)));
    }

    #[test]
    fn test_parse_hunk_range() {
        assert_eq!(
            parse_hunk_header("@@ -10,3 +20,5 @@ fn foo"),
            Some((20, 24))
        );
    }

    #[test]
    fn test_parse_hunk_deletion_only() {
        assert_eq!(parse_hunk_header("@@ -10,3 +9,0 @@"), None);
    }

    #[test]
    fn test_parse_diff_output_basic() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
index abc123..def456 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,5 @@
+use std::io;
+
 fn main() {
-    println!("old");
+    println!("new");
+    io::stdout().flush().unwrap();
 }
"#;
        let result = parse_diff_output(diff).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].path, "src/main.rs");
        assert_eq!(result[0].status, DiffStatus::Modified);
        assert!(result[0].changed_lines.contains(1));
        assert!(result[0].changed_lines.contains(5));
    }

    #[test]
    fn test_parse_diff_new_file() {
        let diff = r#"diff --git a/new.rs b/new.rs
new file mode 100644
index 0000000..abc123
--- /dev/null
+++ b/new.rs
@@ -0,0 +1,3 @@
+fn new_func() {
+    todo!()
+}
"#;
        let result = parse_diff_output(diff).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].status, DiffStatus::Added);
        assert_eq!(result[0].changed_lines.len(), 3);
    }

    #[test]
    fn test_parse_diff_rename() {
        let diff = r#"diff --git a/old.rs b/new.rs
similarity index 90%
rename from old.rs
rename to new.rs
--- a/old.rs
+++ b/new.rs
@@ -1 +1 @@
-old line
+new line
"#;
        let result = parse_diff_output(diff).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].status, DiffStatus::Renamed);
        assert_eq!(result[0].old_path.as_deref(), Some("old.rs"));
        assert_eq!(result[0].path, "new.rs");
    }

    #[test]
    fn test_diff_cache_key_stable() {
        let a = diff_cache_key("abc", "def");
        let b = diff_cache_key("abc", "def");
        let c = diff_cache_key("def", "abc");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn test_diff_cache_path_ext() {
        let p = diff_cache_path("abc", "def");
        assert!(p.to_string_lossy().contains(".covy/state/diff-cache"));
        assert_eq!(p.extension().and_then(|e| e.to_str()), Some("diff"));
    }

    #[test]
    fn git_spawn_not_found_maps_to_typed_variant() {
        let error = git_spawn_error("diff", std::io::Error::from(std::io::ErrorKind::NotFound));

        assert!(matches!(error, DiffyError::GitNotFound { .. }));
    }

    #[test]
    fn git_spawn_other_failure_maps_to_typed_variant() {
        let error = git_spawn_error(
            "rev-parse",
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );

        assert!(matches!(error, DiffyError::GitSpawn { .. }));
    }
}
