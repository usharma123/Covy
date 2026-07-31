//! Bounded, no-follow resolution of missing repository-path suffixes.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_TRAVERSAL_DEPTH: usize = 64;
const MAX_TRAVERSAL_ENTRIES: usize = 100_000;

#[derive(Clone, Copy)]
struct TraversalLimits {
    max_depth: usize,
    max_entries: usize,
}

pub(crate) enum SuffixResolution {
    Unique(String),
    MissingOrAmbiguous,
    Exhausted,
}

struct Traversal<'a> {
    root: &'a Path,
    needle: &'a str,
    needle_suffix: String,
    limits: TraversalLimits,
    visited: BTreeSet<PathBuf>,
    matches: BTreeSet<String>,
    entries_seen: usize,
    exhausted: bool,
}

impl Traversal<'_> {
    fn visit(&mut self, current: &Path, depth: usize) {
        if self.exhausted || self.matches.len() > 1 {
            return;
        }
        if depth > self.limits.max_depth {
            self.exhausted = true;
            return;
        }
        let Ok(metadata) = fs::symlink_metadata(current) else {
            return;
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return;
        }
        let Ok(canonical) = fs::canonicalize(current) else {
            return;
        };
        if !canonical.starts_with(self.root) || !self.visited.insert(canonical) {
            return;
        }
        let Ok(entries) = fs::read_dir(current) else {
            return;
        };
        for entry in entries.flatten() {
            self.entries_seen = self.entries_seen.saturating_add(1);
            if self.entries_seen > self.limits.max_entries {
                self.exhausted = true;
                return;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if depth == 0
                && file_type.is_dir()
                && matches!(entry.file_name().to_str(), Some(".git" | ".packet28"))
            {
                continue;
            }
            if file_type.is_dir() {
                self.visit(&path, depth.saturating_add(1));
                if self.exhausted || self.matches.len() > 1 {
                    return;
                }
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Ok(stripped) = path.strip_prefix(self.root) else {
                continue;
            };
            let normalized = stripped.to_string_lossy().replace('\\', "/");
            if normalized == self.needle || normalized.ends_with(&self.needle_suffix) {
                self.matches.insert(normalized);
                if self.matches.len() > 1 {
                    return;
                }
            }
        }
    }
}

pub(crate) fn resolve_capture_path_suffix(root: &Path, needle: &str) -> SuffixResolution {
    resolve_with_limits(
        root,
        needle,
        TraversalLimits {
            max_depth: MAX_TRAVERSAL_DEPTH,
            max_entries: MAX_TRAVERSAL_ENTRIES,
        },
    )
}

fn resolve_with_limits(root: &Path, needle: &str, limits: TraversalLimits) -> SuffixResolution {
    let Ok(canonical_root) = fs::canonicalize(root) else {
        return SuffixResolution::MissingOrAmbiguous;
    };
    let mut traversal = Traversal {
        root: &canonical_root,
        needle,
        needle_suffix: format!("/{needle}"),
        limits,
        visited: BTreeSet::new(),
        matches: BTreeSet::new(),
        entries_seen: 0,
        exhausted: false,
    };
    traversal.visit(&canonical_root, 0);
    if traversal.exhausted {
        SuffixResolution::Exhausted
    } else {
        let mut matches = traversal.matches.into_iter();
        match (matches.next(), matches.next()) {
            (Some(unique), None) => SuffixResolution::Unique(unique),
            _ => SuffixResolution::MissingOrAmbiguous,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_with_limits, SuffixResolution, TraversalLimits};
    use std::fs;

    use tempfile::tempdir;

    #[test]
    fn resolution_fails_closed_when_entry_budget_is_exhausted() {
        let root = tempdir().unwrap();
        fs::write(root.path().join("needle.rs"), "outside budget").unwrap();

        let result = resolve_with_limits(
            root.path(),
            "needle.rs",
            TraversalLimits {
                max_depth: usize::MAX,
                max_entries: 0,
            },
        );

        assert!(matches!(result, SuffixResolution::Exhausted));
    }

    #[test]
    fn resolution_fails_closed_when_depth_budget_is_exhausted() {
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join("nested")).unwrap();

        let result = resolve_with_limits(
            root.path(),
            "missing.rs",
            TraversalLimits {
                max_depth: 0,
                max_entries: usize::MAX,
            },
        );

        assert!(matches!(result, SuffixResolution::Exhausted));
    }

    #[test]
    fn resolution_skips_packet28_control_state() {
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join(".packet28")).unwrap();
        fs::write(root.path().join(".packet28/needle.rs"), "control state").unwrap();

        let result = resolve_with_limits(
            root.path(),
            "needle.rs",
            TraversalLimits {
                max_depth: usize::MAX,
                max_entries: usize::MAX,
            },
        );

        assert!(matches!(result, SuffixResolution::MissingOrAmbiguous));
    }

    #[cfg(unix)]
    #[test]
    fn resolution_does_not_follow_a_directory_symlink_cycle() {
        use std::os::unix::fs::symlink;

        let root = tempdir().unwrap();
        symlink(root.path(), root.path().join("cycle")).unwrap();

        let result = resolve_with_limits(
            root.path(),
            "missing.rs",
            TraversalLimits {
                max_depth: usize::MAX,
                max_entries: usize::MAX,
            },
        );

        assert!(matches!(result, SuffixResolution::MissingOrAmbiguous));
    }
}
