use crate::types::{CommandReducerSpec, CommandReduction};

pub fn classify_fs_command(command: &str, argv: &[String]) -> Option<CommandReducerSpec> {
    let program = argv.first()?.as_str();
    let (canonical_kind, operation_kind) = match program {
        "ls" if classify_ls(argv) => ("fs_ls", suite_packet_core::ToolOperationKind::Read),
        "tree" if classify_tree(argv) => ("fs_tree", suite_packet_core::ToolOperationKind::Search),
        "find" if classify_find(argv) => ("fs_find", suite_packet_core::ToolOperationKind::Search),
        "cat" if classify_cat(argv) => ("fs_cat", suite_packet_core::ToolOperationKind::Read),
        "head" if classify_head_tail(argv, false) => {
            ("fs_head", suite_packet_core::ToolOperationKind::Read)
        }
        "tail" if classify_head_tail(argv, true) => {
            ("fs_tail", suite_packet_core::ToolOperationKind::Read)
        }
        "sed" if classify_sed(argv) => ("fs_sed", suite_packet_core::ToolOperationKind::Read),
        "diff" if classify_diff(argv) => ("fs_diff", suite_packet_core::ToolOperationKind::Diff),
        "wc" if classify_wc(argv) => ("fs_wc", suite_packet_core::ToolOperationKind::Read),
        "grep" if classify_grep(argv) => ("fs_grep", suite_packet_core::ToolOperationKind::Search),
        "rg" if classify_grep(argv) => ("fs_rg", suite_packet_core::ToolOperationKind::Search),
        _ => return None,
    };
    let paths = if canonical_kind == "fs_tree" {
        extract_tree_paths(argv)
    } else {
        argv.iter()
            .skip(1)
            .filter(|arg| !arg.starts_with('-') && looks_like_path(arg))
            .cloned()
            .collect::<Vec<_>>()
    };
    let equivalence_key = match canonical_kind {
        "fs_cat" => paths.first().map(|path| format!("read:{path}")),
        "fs_head" => paths
            .first()
            .map(|path| format!("read:{}:{}", path, parse_head_count(argv).unwrap_or(10))),
        "fs_sed" => paths.first().and_then(|path| {
            parse_sed_region(argv).map(|(start, end)| format!("read:{path}:{start}-{end}"))
        }),
        "fs_find" => Some(format!(
            "glob:{}",
            argv.iter().skip(1).cloned().collect::<Vec<_>>().join(" ")
        )),
        "fs_tree" => Some(format!(
            "tree:{}",
            argv.iter().skip(1).cloned().collect::<Vec<_>>().join(" ")
        )),
        _ => None,
    };
    Some(CommandReducerSpec {
        family: "fs".to_string(),
        canonical_kind: canonical_kind.to_string(),
        packet_type: "packet28.hook.fs.v2".to_string(),
        operation_kind,
        command: command.to_string(),
        argv: argv.to_vec(),
        cache_fingerprint: fingerprint("fs", canonical_kind, argv),
        cacheable: true,
        mutation: false,
        paths,
        equivalence_key,
    })
}

pub fn reduce_fs_command(
    spec: &CommandReducerSpec,
    stdout: &str,
    stderr: &str,
    exit_code: i32,
) -> CommandReduction {
    let failed = exit_code != 0;
    let lines = nonempty_lines(stdout).len();
    let summary = match spec.canonical_kind.as_str() {
        "fs_ls" => {
            let target = spec
                .paths
                .first()
                .cloned()
                .unwrap_or_else(|| ".".to_string());
            format!(
                "ls listed {lines} entr{suffix} in {target}",
                suffix = if lines == 1 { "y" } else { "ies" }
            )
        }
        "fs_find" => {
            let target = spec
                .paths
                .first()
                .cloned()
                .unwrap_or_else(|| ".".to_string());
            format!("find matched {lines} path(s) under {target}")
        }
        "fs_tree" => {
            let target = spec
                .paths
                .first()
                .cloned()
                .unwrap_or_else(|| ".".to_string());
            let counts = parse_tree_counts(stdout);
            match counts {
                Some((dirs, files)) => {
                    format!("tree listed {dirs} dir(s), {files} file(s) under {target}")
                }
                None => format!("tree listed {lines} line(s) under {target}"),
            }
        }
        "fs_cat" | "fs_head" | "fs_tail" | "fs_sed" => {
            let target = spec
                .paths
                .first()
                .cloned()
                .unwrap_or_else(|| "file".to_string());
            match spec.canonical_kind.as_str() {
                "fs_head" => format!(
                    "head {target}{}{}",
                    parse_head_count(&spec.argv)
                        .map(|count| format!(" lines 1-{count}"))
                        .unwrap_or_default(),
                    preview_suffix(stdout)
                ),
                "fs_tail" => format!(
                    "tail {target}{}{}",
                    parse_head_count(&spec.argv)
                        .map(|count| format!(" last {count} line(s)"))
                        .unwrap_or_default(),
                    preview_suffix(stdout)
                ),
                "fs_sed" => {
                    if let Some((start, end)) = parse_sed_region(&spec.argv) {
                        format!("sed {target} lines {start}-{end}{}", preview_suffix(stdout))
                    } else {
                        format!(
                            "sed {target} returned {lines} line(s){}",
                            preview_suffix(stdout)
                        )
                    }
                }
                _ => format!(
                    "cat {target}{}{}",
                    line_count_suffix(lines),
                    preview_suffix(stdout)
                ),
            }
        }
        "fs_diff" => {
            let compared = spec.paths.iter().take(2).cloned().collect::<Vec<_>>();
            if lines > 0 {
                if compared.len() == 2 {
                    format!(
                        "diff compared {} and {} ({lines} output line(s))",
                        compared[0], compared[1]
                    )
                } else {
                    format!("diff produced {lines} output line(s)")
                }
            } else {
                "diff completed".to_string()
            }
        }
        "fs_grep" | "fs_rg" => {
            let program = spec.argv.first().map(String::as_str).unwrap_or("grep");
            if failed {
                first_nonempty_line(stderr).unwrap_or_else(|| format!("{program} failed"))
            } else {
                format!(
                    "{program} matched {lines} line(s){}",
                    preview_suffix(stdout)
                )
            }
        }
        "fs_wc" => {
            if failed {
                first_nonempty_line(stderr).unwrap_or_else(|| "wc failed".to_string())
            } else {
                summarize_wc(stdout, &detect_wc_mode(&spec.argv))
            }
        }
        _ => {
            let command = spec
                .argv
                .first()
                .cloned()
                .unwrap_or_else(|| "<unknown command>".to_string());
            first_nonempty_line(stdout).unwrap_or_else(|| format!("{command} completed"))
        }
    };
    let mut regions = Vec::new();
    if let Some(path) = spec.paths.first() {
        match spec.canonical_kind.as_str() {
            "fs_cat" => {
                let end = lines.max(1);
                regions.push(format!("{path}:1-{end}"));
            }
            "fs_head" => {
                let end = lines.max(1);
                regions.push(format!("{path}:1-{end}"));
            }
            "fs_sed" => {
                if let Some((start, end)) = parse_sed_region(&spec.argv) {
                    regions.push(format!("{path}:{start}-{end}"));
                }
            }
            _ => {}
        }
    }
    CommandReduction {
        family: spec.family.clone(),
        canonical_kind: spec.canonical_kind.clone(),
        packet_type: spec.packet_type.clone(),
        operation_kind: spec.operation_kind,
        summary: if failed {
            first_nonempty_line(stderr).unwrap_or(summary)
        } else {
            summary
        },
        compact_preview: if spec.canonical_kind == "fs_wc" && !failed {
            compact_wc_output(stdout, &detect_wc_mode(&spec.argv))
        } else {
            String::new()
        },
        paths: spec.paths.clone(),
        regions,
        symbols: Vec::new(),
        failed,
        error_class: failed.then(|| "fs_error".to_string()),
        error_message: failed.then(|| compact(stderr, 200)),
        retryable: failed.then_some(false),
        exit_code,
        cache_fingerprint: spec.cache_fingerprint.clone(),
        cacheable: spec.cacheable,
        mutation: spec.mutation,
        equivalence_key: spec.equivalence_key.clone(),
    }
}

fn classify_ls(argv: &[String]) -> bool {
    !contains_any(argv, &["-R", "--recursive", "--dired"])
}

fn classify_tree(argv: &[String]) -> bool {
    !contains_any(
        argv,
        &["-J", "--json", "-X", "--xml", "-H", "--du", "--fromfile"],
    )
}

fn extract_tree_paths(argv: &[String]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut skip_next = false;
    for arg in argv.iter().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        match arg.as_str() {
            "-L" | "-P" | "-I" | "-o" => {
                skip_next = true;
            }
            value if value.starts_with("--filelimit=") || value.starts_with("-") => {}
            value => paths.push(value.to_string()),
        }
    }
    paths
}

fn classify_grep(argv: &[String]) -> bool {
    argv.len() >= 2
        && !contains_any(argv, &["--binary-files", "-z", "--null-data"])
        && !has_grep_extraction_mode(argv)
}

fn has_grep_extraction_mode(argv: &[String]) -> bool {
    argv.iter().skip(1).any(|arg| match arg.as_str() {
        "-o"
        | "--only-matching"
        | "-c"
        | "--count"
        | "-l"
        | "--files-with-matches"
        | "-L"
        | "--files-without-match"
        | "-q"
        | "--quiet"
        | "--silent" => true,
        value if value.starts_with("--only-matching=") || value.starts_with("--count=") => true,
        value if value.starts_with('-') && !value.starts_with("--") => value
            .trim_start_matches('-')
            .chars()
            .any(|ch| matches!(ch, 'o' | 'c' | 'l' | 'L' | 'q')),
        _ => false,
    })
}

fn classify_find(argv: &[String]) -> bool {
    !contains_any(
        argv,
        &[
            "-exec", "-execdir", "-ok", "-okdir", "-delete", "-printf", "-fprintf", "-ls",
        ],
    )
}

fn classify_cat(argv: &[String]) -> bool {
    !contains_any(argv, &["-v", "-A", "--show-all", "--show-nonprinting"])
}

fn classify_head_tail(argv: &[String], tail: bool) -> bool {
    if contains_any(argv, &["-c", "--bytes"]) {
        return false;
    }
    if tail && contains_any(argv, &["-f", "--follow", "--retry", "-F"]) {
        return false;
    }
    parse_head_count(argv).is_some() || argv.len() >= 2
}

fn classify_sed(argv: &[String]) -> bool {
    if contains_any(argv, &["-i", "--in-place", "-e", "-f"]) {
        return false;
    }
    parse_sed_region(argv).is_some()
}

fn classify_diff(argv: &[String]) -> bool {
    if contains_any(
        argv,
        &[
            "--git",
            "--patch",
            "-p",
            "--raw",
            "--word-diff",
            "--side-by-side",
        ],
    ) {
        return false;
    }
    argv.iter()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .count()
        >= 2
}

fn classify_wc(argv: &[String]) -> bool {
    !contains_any(argv, &["--files0-from"])
        && argv
            .iter()
            .skip(1)
            .any(|arg| !arg.starts_with('-') && looks_like_path(arg))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WcMode {
    Full,
    Lines,
    Words,
    Bytes,
    Chars,
    Mixed,
}

fn detect_wc_mode(argv: &[String]) -> WcMode {
    let mut has_l = false;
    let mut has_w = false;
    let mut has_c = false;
    let mut has_m = false;
    let mut count = 0usize;
    for arg in argv.iter().skip(1).filter(|arg| arg.starts_with('-')) {
        for ch in arg.trim_start_matches('-').chars() {
            match ch {
                'l' => {
                    has_l = true;
                    count += 1;
                }
                'w' => {
                    has_w = true;
                    count += 1;
                }
                'c' => {
                    has_c = true;
                    count += 1;
                }
                'm' => {
                    has_m = true;
                    count += 1;
                }
                _ => {}
            }
        }
    }
    match (count, has_l, has_w, has_c, has_m) {
        (0, _, _, _, _) => WcMode::Full,
        (1, true, _, _, _) => WcMode::Lines,
        (1, _, true, _, _) => WcMode::Words,
        (1, _, _, true, _) => WcMode::Bytes,
        (1, _, _, _, true) => WcMode::Chars,
        _ => WcMode::Mixed,
    }
}

fn contains_any(argv: &[String], denied: &[&str]) -> bool {
    argv.iter().any(|arg| {
        denied
            .iter()
            .any(|denied| arg == denied || arg.starts_with(&format!("{denied}=")))
    })
}

fn looks_like_path(value: &str) -> bool {
    !(value == "." || value == "..")
        && (value.contains('/')
            || value.starts_with('.')
            || value
                .chars()
                .any(|ch| ch.is_ascii_alphabetic() && value.contains('.')))
}

fn first_nonempty_line(value: &str) -> Option<String> {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)
}

fn nonempty_lines(value: &str) -> Vec<&str> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect()
}

fn parse_tree_counts(stdout: &str) -> Option<(usize, usize)> {
    let line = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.contains("director") && line.contains("file"))?;
    let mut dirs = None;
    let mut files = None;
    let parts = line.split(',').map(str::trim).collect::<Vec<_>>();
    for part in parts {
        let mut words = part.split_whitespace();
        let count = words.next()?.parse::<usize>().ok()?;
        let label = words.next().unwrap_or_default();
        if label.starts_with("director") {
            dirs = Some(count);
        } else if label.starts_with("file") {
            files = Some(count);
        }
    }
    Some((dirs?, files?))
}

fn parse_head_count(argv: &[String]) -> Option<usize> {
    let mut idx = 1;
    while idx < argv.len() {
        let arg = argv[idx].as_str();
        if let Some(value) = arg.strip_prefix("--lines=") {
            return value.parse::<usize>().ok();
        }
        if arg == "-n" || arg == "--lines" {
            return argv.get(idx + 1)?.parse::<usize>().ok();
        }
        if let Some(value) = arg.strip_prefix('-') {
            if !value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit()) {
                return value.parse::<usize>().ok();
            }
        }
        idx += 1;
    }
    Some(10)
}

fn parse_sed_region(argv: &[String]) -> Option<(usize, usize)> {
    let expression = argv
        .iter()
        .skip(1)
        .find(|arg| !arg.starts_with('-'))
        .map(String::as_str)?;
    if !expression.ends_with('p') {
        return None;
    }
    let range = expression.strip_suffix('p')?;
    if let Some((start, end)) = range.split_once(',') {
        return Some((start.parse::<usize>().ok()?, end.parse::<usize>().ok()?));
    }
    let line = range.parse::<usize>().ok()?;
    Some((line, line))
}

fn line_count_suffix(lines: usize) -> String {
    format!(" ({lines} line(s))")
}

fn preview_suffix(stdout: &str) -> String {
    first_nonempty_line(stdout)
        .map(|line| format!(": {}", compact(&line, 60)))
        .unwrap_or_default()
}

fn compact(value: &str, limit: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let char_count = compact.chars().count();
    if char_count <= limit {
        compact
    } else if limit <= 3 {
        "...".to_string()
    } else {
        let shortened = compact
            .chars()
            .take(limit.saturating_sub(3))
            .collect::<String>();
        format!("{shortened}...")
    }
}

fn summarize_wc(stdout: &str, mode: &WcMode) -> String {
    let compact = compact_wc_output(stdout, mode);
    let lines = compact
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    if lines <= 1 {
        format!("wc {}", compact.trim())
    } else {
        format!("wc counted {lines} row(s)")
    }
}

fn compact_wc_output(stdout: &str, mode: &WcMode) -> String {
    let lines = stdout
        .trim()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return String::new();
    }
    if lines.len() == 1 {
        return format_wc_line(lines[0], mode);
    }
    let paths = lines
        .iter()
        .filter_map(|line| line.split_whitespace().last())
        .filter(|path| *path != "total")
        .collect::<Vec<_>>();
    let prefix = common_path_prefix(&paths);
    lines
        .iter()
        .map(|line| format_wc_multi_line(line, mode, &prefix))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_wc_line(line: &str, mode: &WcMode) -> String {
    let parts = line.split_whitespace().collect::<Vec<_>>();
    match mode {
        WcMode::Lines | WcMode::Words | WcMode::Bytes | WcMode::Chars => {
            parts.first().copied().unwrap_or_default().to_string()
        }
        WcMode::Full => {
            if parts.len() >= 3 {
                format!("{}L {}W {}B", parts[0], parts[1], parts[2])
            } else {
                line.trim().to_string()
            }
        }
        WcMode::Mixed => strip_wc_path(&parts).join(" "),
    }
}

fn format_wc_multi_line(line: &str, mode: &WcMode, prefix: &str) -> String {
    let parts = line.split_whitespace().collect::<Vec<_>>();
    if parts.is_empty() {
        return String::new();
    }
    let is_total = parts.last() == Some(&"total");
    match mode {
        WcMode::Lines | WcMode::Words | WcMode::Bytes | WcMode::Chars => {
            let count = parts.first().copied().unwrap_or("0");
            if is_total {
                format!("Σ {count}")
            } else {
                let name = strip_prefix(parts.last().copied().unwrap_or_default(), prefix);
                format!("{count} {name}")
            }
        }
        WcMode::Full => {
            if is_total {
                format!(
                    "Σ {}L {}W {}B",
                    parts.first().copied().unwrap_or("0"),
                    parts.get(1).copied().unwrap_or("0"),
                    parts.get(2).copied().unwrap_or("0")
                )
            } else if parts.len() >= 4 {
                let name = strip_prefix(parts[3], prefix);
                format!("{}L {}W {}B {name}", parts[0], parts[1], parts[2])
            } else {
                line.trim().to_string()
            }
        }
        WcMode::Mixed => {
            if is_total {
                format!("Σ {}", parts[..parts.len().saturating_sub(1)].join(" "))
            } else if parts.len() >= 2
                && parts
                    .last()
                    .is_some_and(|value| value.parse::<u64>().is_err())
            {
                let name = strip_prefix(parts.last().copied().unwrap_or_default(), prefix);
                format!("{} {name}", parts[..parts.len() - 1].join(" "))
            } else {
                parts.join(" ")
            }
        }
    }
}

fn strip_wc_path<'a>(parts: &[&'a str]) -> Vec<&'a str> {
    if parts.len() >= 2
        && parts
            .last()
            .is_some_and(|value| value.parse::<u64>().is_err())
    {
        parts[..parts.len() - 1].to_vec()
    } else {
        parts.to_vec()
    }
}

fn common_path_prefix(paths: &[&str]) -> String {
    if paths.len() <= 1 {
        return String::new();
    }
    let Some(idx) = paths[0].rfind('/') else {
        return String::new();
    };
    let mut prefix = paths[0][..=idx].to_string();
    while !prefix.is_empty() {
        if paths.iter().all(|path| path.starts_with(&prefix)) {
            return prefix;
        }
        let trimmed = prefix.trim_end_matches('/');
        let Some(idx) = trimmed.rfind('/') else {
            return String::new();
        };
        prefix.truncate(idx + 1);
    }
    String::new()
}

fn strip_prefix<'a>(path: &'a str, prefix: &str) -> &'a str {
    if prefix.is_empty() {
        path
    } else {
        path.strip_prefix(prefix).unwrap_or(path)
    }
}

fn fingerprint(family: &str, kind: &str, argv: &[String]) -> String {
    crate::cache_fingerprint(family, kind, argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_sed_declines_mutating_shapes() {
        let argv = vec!["sed", "-i", "1,4p", "src/lib.rs"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert!(classify_fs_command("sed -i 1,4p src/lib.rs", &argv).is_none());
    }

    #[test]
    fn reduce_head_adds_region() {
        let argv = vec!["head", "-n", "3", "README.md"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_fs_command("head -n 3 README.md", &argv).unwrap();
        let reduction = reduce_fs_command(&spec, "a\nb\nc\n", "", 0);
        assert_eq!(reduction.summary, "head README.md lines 1-3: a");
        assert_eq!(reduction.regions, vec!["README.md:1-3".to_string()]);
    }

    #[test]
    fn reduce_tree_summarizes_footer_counts() {
        let argv = vec!["tree", "-L", "2", "src"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_fs_command("tree -L 2 src", &argv).unwrap();
        let reduction = reduce_fs_command(
            &spec,
            "src\n├── lib.rs\n└── bin\n    └── cli.rs\n\n1 directory, 2 files\n",
            "",
            0,
        );
        assert_eq!(
            reduction.summary,
            "tree listed 1 dir(s), 2 file(s) under src"
        );
        assert_eq!(reduction.canonical_kind, "fs_tree");
    }

    #[test]
    fn reduce_wc_single_file_strips_path_and_padding() {
        let argv = vec!["wc", "scripts/find_duplicate_attrs.py"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_fs_command("wc scripts/find_duplicate_attrs.py", &argv).unwrap();
        let reduction = reduce_fs_command(
            &spec,
            "      30      96     978 scripts/find_duplicate_attrs.py\n",
            "",
            0,
        );
        assert_eq!(reduction.canonical_kind, "fs_wc");
        assert_eq!(reduction.summary, "wc 30L 96W 978B");
        assert_eq!(reduction.compact_preview, "30L 96W 978B");
    }

    #[test]
    fn reduce_wc_lines_only_returns_count() {
        let argv = vec!["wc", "-l", "src/main.rs"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_fs_command("wc -l src/main.rs", &argv).unwrap();
        let reduction = reduce_fs_command(&spec, "      30 src/main.rs\n", "", 0);
        assert_eq!(reduction.summary, "wc 30");
        assert_eq!(reduction.compact_preview, "30");
    }

    #[test]
    fn reduce_wc_multi_file_strips_common_prefix() {
        let argv = vec!["wc", "-l", "src/main.rs", "src/lib.rs"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_fs_command("wc -l src/main.rs src/lib.rs", &argv).unwrap();
        let output = "      30 src/main.rs\n      50 src/lib.rs\n      80 total\n";
        let reduction = reduce_fs_command(&spec, output, "", 0);
        assert_eq!(reduction.summary, "wc counted 3 row(s)");
        assert_eq!(reduction.compact_preview, "30 main.rs\n50 lib.rs\nΣ 80");
    }
}
