use anyhow::Result;

use crate::dotnet::{classify_dotnet_command, reduce_dotnet_command};
use crate::fs::{classify_fs_command, reduce_fs_command};
use crate::git::{classify_git_command, reduce_git_command};
use crate::github::{classify_github_command, reduce_github_command};
use crate::go::{classify_go_command, reduce_go_command};
use crate::infra::{classify_infra_command, reduce_infra_command};
use crate::javascript::{classify_javascript_command, reduce_javascript_command};
use crate::jvm::{classify_jvm_command, reduce_jvm_command};
use crate::python::{classify_python_command, reduce_python_command};
use crate::ruby::{classify_ruby_command, reduce_ruby_command};
use crate::rust::{classify_rust_command, reduce_rust_command};
use crate::types::{CommandReducerFamily, CommandReducerSpec, CommandReduction};

pub fn classify_command(command: &str) -> Option<CommandReducerSpec> {
    let normalized = strip_supported_trailing_redirects(command.trim());
    let trimmed = strip_supported_postprocess(&normalized);
    let trimmed = trimmed.trim();
    if trimmed.is_empty()
        || contains_shell_composition(trimmed)
        || contains_shell_expansion(trimmed)
    {
        return None;
    }
    let argv = shell_words::split(trimmed).ok()?;
    let (_env_assignments, argv) = consume_leading_env_assignments(argv);
    if argv.is_empty() {
        return None;
    }
    classify_command_argv(trimmed, &argv)
}

pub fn classify_command_argv(command: &str, argv: &[String]) -> Option<CommandReducerSpec> {
    let first = argv.first()?.as_str();
    if is_python_interpreter(first) {
        return classify_python_command(command, argv);
    }
    match argv.first()?.as_str() {
        "git" | "yadm" | "gt" => classify_git_command(command, argv),
        "ls" | "tree" | "find" | "cat" | "head" | "tail" | "sed" | "diff" | "wc" | "grep"
        | "rg" => classify_fs_command(command, argv),
        "cargo" => classify_rust_command(command, argv),
        "gh" | "glab" => classify_github_command(command, argv),
        "go" | "golangci-lint" => classify_go_command(command, argv),
        "docker" | "kubectl" | "curl" | "wget" | "aws" | "psql" => {
            classify_infra_command(command, argv)
        }
        "pytest" | "ruff" | "pip" | "pip3" | "uv" | "mypy" => {
            classify_python_command(command, argv)
        }
        "npm" | "pnpm" | "yarn" | "npx" | "pnpx" | "tsc" | "eslint" | "biome" | "lint"
        | "vitest" | "jest" | "prettier" | "next" | "prisma" | "playwright" => {
            classify_javascript_command(command, argv)
        }
        "gradle" | "gradlew" | "./gradlew" | "gradlew.bat" => classify_jvm_command(command, argv),
        "bundle" | "rspec" | "rubocop" | "ruby" | "rails" | "rake" => {
            classify_ruby_command(command, argv)
        }
        "dotnet" => classify_dotnet_command(command, argv),
        _ => None,
    }
}

fn is_python_interpreter(value: &str) -> bool {
    value == "python"
        || value == "python3"
        || value.strip_prefix("python").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
        })
}

pub fn reduce_command_output(
    spec: &CommandReducerSpec,
    stdout: &str,
    stderr: &str,
    exit_code: i32,
) -> Result<CommandReduction> {
    let family = parse_family(&spec.family)?;
    Ok(match family {
        CommandReducerFamily::Git => reduce_git_command(spec, stdout, stderr, exit_code),
        CommandReducerFamily::Fs => reduce_fs_command(spec, stdout, stderr, exit_code),
        CommandReducerFamily::Rust => reduce_rust_command(spec, stdout, stderr, exit_code),
        CommandReducerFamily::Github => reduce_github_command(spec, stdout, stderr, exit_code),
        CommandReducerFamily::Go => reduce_go_command(spec, stdout, stderr, exit_code),
        CommandReducerFamily::Infra => reduce_infra_command(spec, stdout, stderr, exit_code),
        CommandReducerFamily::Ruby => reduce_ruby_command(spec, stdout, stderr, exit_code),
        CommandReducerFamily::Dotnet => reduce_dotnet_command(spec, stdout, stderr, exit_code),
        CommandReducerFamily::Python => reduce_python_command(spec, stdout, stderr, exit_code),
        CommandReducerFamily::Javascript => {
            reduce_javascript_command(spec, stdout, stderr, exit_code)
        }
        CommandReducerFamily::Jvm => reduce_jvm_command(spec, stdout, stderr, exit_code),
    })
}

fn parse_family(value: &str) -> Result<CommandReducerFamily> {
    Ok(match value {
        "git" => CommandReducerFamily::Git,
        "fs" => CommandReducerFamily::Fs,
        "rust" => CommandReducerFamily::Rust,
        "github" => CommandReducerFamily::Github,
        "go" => CommandReducerFamily::Go,
        "infra" => CommandReducerFamily::Infra,
        "python" => CommandReducerFamily::Python,
        "javascript" => CommandReducerFamily::Javascript,
        "jvm" => CommandReducerFamily::Jvm,
        "ruby" => CommandReducerFamily::Ruby,
        "dotnet" => CommandReducerFamily::Dotnet,
        other => anyhow::bail!("unsupported reducer family '{other}'"),
    })
}

fn looks_like_env_assignment(arg: &str) -> bool {
    arg.contains('=')
        && !arg.starts_with('=')
        && arg
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
}

/// Consumes leading `KEY=VALUE` arguments and returns the remaining command.
///
/// The returned command reuses the input vector and its owned argument strings,
/// so routing a command does not clone the command tail.
pub fn consume_leading_env_assignments(
    mut argv: Vec<String>,
) -> (Vec<(String, String)>, Vec<String>) {
    let prefix_len = argv
        .iter()
        .take_while(|arg| looks_like_env_assignment(arg))
        .count();
    let assignments = argv
        .drain(..prefix_len)
        .map(|mut assignment| {
            let equals = assignment
                .find('=')
                .expect("validated environment assignment must contain '='");
            let mut value = assignment.split_off(equals);
            value.remove(0);
            assignment.truncate(assignment.trim_end().len());
            (assignment, value)
        })
        .collect();
    (assignments, argv)
}

fn strip_supported_trailing_redirects(command: &str) -> String {
    let mut trimmed = command.trim().to_string();
    loop {
        let Some(stripped) = strip_one_trailing_redirect(&trimmed) else {
            break;
        };
        trimmed = stripped.trim_end().to_string();
    }
    trimmed
}

fn strip_one_trailing_redirect(command: &str) -> Option<&str> {
    const REDIRECT_SUFFIXES: &[&str] = &[
        "2>&1",
        "1>&2",
        ">/dev/null",
        "1>/dev/null",
        "2>/dev/null",
        "1> /dev/null",
        "2> /dev/null",
        "> /dev/null",
    ];
    REDIRECT_SUFFIXES
        .iter()
        .find_map(|suffix| command.strip_suffix(suffix))
}

fn strip_supported_postprocess(command: &str) -> String {
    let Some(pipe_idx) = find_last_top_level_pipe(command) else {
        return command.trim().to_string();
    };
    let lhs = command[..pipe_idx].trim();
    let rhs = command[pipe_idx + 1..].trim();
    if is_supported_postprocess(rhs) {
        lhs.to_string()
    } else {
        command.trim().to_string()
    }
}

fn find_last_top_level_pipe(command: &str) -> Option<usize> {
    let bytes = command.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut last_pipe = None;
    let mut idx = 0usize;
    while idx < bytes.len() {
        let ch = bytes[idx] as char;
        if ch == '\'' && !in_double {
            in_single = !in_single;
        } else if ch == '"' && !in_single {
            in_double = !in_double;
        } else if ch == '|'
            && !in_single
            && !in_double
            && bytes.get(idx + 1).map(|next| *next != b'|').unwrap_or(true)
            && idx > 0
            && bytes[idx - 1] != b'|'
        {
            last_pipe = Some(idx);
        }
        idx += 1;
    }
    last_pipe
}

fn is_supported_postprocess(command: &str) -> bool {
    let Ok(argv) = shell_words::split(command) else {
        return false;
    };
    match argv.first().map(String::as_str) {
        Some("head") | Some("tail") => {
            parse_count_flag(argv.iter().skip(1).cloned().collect()).is_some()
        }
        Some("sed") => {
            argv.len() == 3
                && argv.get(1).map(String::as_str) == Some("-n")
                && parse_sed_range(&argv[2]).is_some()
        }
        _ => false,
    }
}

fn parse_count_flag(argv: Vec<String>) -> Option<usize> {
    let mut count = 10usize;
    let mut iter = argv.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-n" => count = iter.next()?.parse().ok()?,
            value
                if value.starts_with('-')
                    && value.len() > 1
                    && value[1..].chars().all(|ch| ch.is_ascii_digit()) =>
            {
                count = value[1..].parse().ok()?;
            }
            value if value.starts_with('-') => {}
            _ => return None,
        }
    }
    Some(count)
}

fn parse_sed_range(value: &str) -> Option<(usize, usize)> {
    let trimmed = value.trim().trim_end_matches('p');
    let (start, end) = trimmed.split_once(',')?;
    Some((start.parse().ok()?, end.parse().ok()?))
}

fn contains_shell_composition(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut idx = 0;
    while idx < bytes.len() {
        let ch = bytes[idx] as char;
        if ch == '\'' && !in_double {
            in_single = !in_single;
        } else if ch == '"' && !in_single {
            in_double = !in_double;
        } else if !in_single && !in_double {
            if matches!(ch, '|' | ';' | '<' | '>' | '`') {
                return true;
            }
            if ch == '&' {
                return true;
            }
            if ch == '$' && bytes.get(idx + 1).is_some_and(|next| *next == b'(') {
                return true;
            }
        }
        idx += 1;
    }
    false
}

fn contains_shell_expansion(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut idx = 0;
    while idx < bytes.len() {
        let ch = bytes[idx] as char;
        if escaped {
            escaped = false;
            idx += 1;
            continue;
        }
        match ch {
            '\\' if !in_single => {
                escaped = true;
            }
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            '$' if !in_single => {
                return true;
            }
            '~' if !in_single && !in_double && idx == 0 => {
                return true;
            }
            '*' | '?' if !in_single && !in_double => {
                return true;
            }
            '[' | '{' if !in_single && !in_double => {
                return true;
            }
            _ => {}
        }
        idx += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declines_shell_composition() {
        assert!(classify_command("cargo test 2>&1 | grep FAIL").is_none());
    }

    #[test]
    fn declines_shell_expansion() {
        assert!(classify_command("git diff src/*.rs").is_none());
        assert!(classify_command("cargo test $CRATE").is_none());
        assert!(classify_command("~/bin/tool").is_none());
    }

    #[test]
    fn accepts_env_assignments_and_trailing_redirects() {
        assert!(classify_command("FOO=1 cargo test").is_some());
        assert!(classify_command("cargo test -q 2>&1").is_some());
    }

    #[test]
    fn consuming_env_assignments_reuses_the_command_storage() {
        let argv = vec![
            "EMPTY=".to_string(),
            "EQUALS=left=right".to_string(),
            "command".to_string(),
            "cargo".to_string(),
            "test".to_string(),
        ];
        let vector_pointer = argv.as_ptr();
        let vector_capacity = argv.capacity();
        let command_pointer = argv[2].as_ptr();

        let (assignments, command) = consume_leading_env_assignments(argv);

        assert_eq!(
            assignments,
            vec![
                ("EMPTY".to_string(), String::new()),
                ("EQUALS".to_string(), "left=right".to_string()),
            ]
        );
        assert_eq!(command, ["command", "cargo", "test"]);
        assert_eq!(command.as_ptr(), vector_pointer);
        assert_eq!(command.capacity(), vector_capacity);
        assert_eq!(command[0].as_ptr(), command_pointer);
    }

    #[test]
    fn accepts_supported_truncation_postprocesses() {
        assert!(classify_command("git show HEAD | head -n 20").is_some());
        assert!(classify_command("cargo test | tail -n 30").is_some());
        assert!(classify_command("git diff | sed -n '1,20p'").is_some());
    }

    #[test]
    fn representative_mutations_are_never_cacheable() {
        for command in [
            "git switch main",
            "cargo install ripgrep",
            "cargo fmt --all",
            "npx svgo icon.svg",
            "pip install pytest",
            "ruff format src",
            "bundle install",
            "docker run alpine echo hi",
            "kubectl apply -f deploy.yaml",
            "wget https://example.com/archive.tgz",
        ] {
            let spec = classify_command(command).expect(command);
            assert!(spec.mutation, "{command}");
            assert!(!spec.cacheable, "{command}");
        }
    }
}
