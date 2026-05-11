use crate::types::{CommandReducerSpec, CommandReduction};

pub fn classify_ruby_command(command: &str, argv: &[String]) -> Option<CommandReducerSpec> {
    let (canonical_kind, operation_kind) = match argv.first()?.as_str() {
        "rspec" if !contains_any(argv, &["--format", "-f", "--profile"]) => {
            ("ruby_rspec", suite_packet_core::ToolOperationKind::Test)
        }
        "rubocop" if !contains_any(argv, &["--format", "-f", "--auto-correct", "-A", "-a"]) => {
            ("ruby_rubocop", suite_packet_core::ToolOperationKind::Build)
        }
        "bundle" if argv.get(1).is_some_and(|arg| arg == "exec") => match argv.get(2)?.as_str() {
            "rspec" if !contains_any(&argv[2..], &["--format", "-f", "--profile"]) => {
                ("ruby_rspec", suite_packet_core::ToolOperationKind::Test)
            }
            "rubocop"
                if !contains_any(
                    &argv[2..],
                    &["--format", "-f", "--auto-correct", "-A", "-a"],
                ) =>
            {
                ("ruby_rubocop", suite_packet_core::ToolOperationKind::Build)
            }
            "rails"
                if argv
                    .get(3)
                    .is_some_and(|arg| matches!(arg.as_str(), "test" | "spec")) =>
            {
                (
                    "ruby_rails_test",
                    suite_packet_core::ToolOperationKind::Test,
                )
            }
            _ => return None,
        },
        "rails"
            if argv
                .get(1)
                .is_some_and(|arg| matches!(arg.as_str(), "test" | "spec")) =>
        {
            (
                "ruby_rails_test",
                suite_packet_core::ToolOperationKind::Test,
            )
        }
        "ruby" if argv.get(1).is_some_and(|arg| arg.ends_with("_test.rb")) => {
            ("ruby_test_file", suite_packet_core::ToolOperationKind::Test)
        }
        _ => return None,
    };

    Some(CommandReducerSpec {
        family: "ruby".to_string(),
        canonical_kind: canonical_kind.to_string(),
        packet_type: "packet28.hook.ruby.v2".to_string(),
        operation_kind,
        command: command.to_string(),
        argv: argv.to_vec(),
        cache_fingerprint: fingerprint("ruby", canonical_kind, argv),
        cacheable: true,
        mutation: false,
        paths: argv
            .iter()
            .skip(1)
            .filter(|arg| !arg.starts_with('-') && looks_like_path(arg))
            .cloned()
            .collect(),
        equivalence_key: None,
    })
}

pub fn reduce_ruby_command(
    spec: &CommandReducerSpec,
    stdout: &str,
    stderr: &str,
    exit_code: i32,
) -> CommandReduction {
    let failed = exit_code != 0;
    let combined = format!("{stdout}\n{stderr}");
    let summary = match spec.canonical_kind.as_str() {
        "ruby_rspec" => summarize_rspec(&combined, failed),
        "ruby_rubocop" => summarize_rubocop(&combined, failed),
        "ruby_rails_test" | "ruby_test_file" => summarize_rails_test(&combined, failed),
        _ => first_nonempty_line(&combined).unwrap_or_else(|| "ruby command completed".to_string()),
    };

    CommandReduction {
        family: spec.family.clone(),
        canonical_kind: spec.canonical_kind.clone(),
        packet_type: spec.packet_type.clone(),
        operation_kind: spec.operation_kind,
        summary,
        compact_preview: if failed {
            compact_ruby_failures(&combined)
        } else {
            String::new()
        },
        paths: spec.paths.clone(),
        regions: Vec::new(),
        symbols: Vec::new(),
        failed,
        error_class: failed.then(|| "ruby_error".to_string()),
        error_message: failed.then(|| compact(&combined, 220)),
        retryable: failed.then_some(false),
        exit_code,
        cache_fingerprint: spec.cache_fingerprint.clone(),
        cacheable: spec.cacheable,
        mutation: spec.mutation,
        equivalence_key: spec.equivalence_key.clone(),
    }
}

fn summarize_rspec(output: &str, failed: bool) -> String {
    if let Some(line) = output
        .lines()
        .map(str::trim)
        .find(|line| line.contains(" example"))
    {
        return format!("rspec: {line}");
    }
    if failed {
        first_nonempty_line(output).unwrap_or_else(|| "rspec failed".to_string())
    } else {
        first_nonempty_line(output).unwrap_or_else(|| "rspec completed".to_string())
    }
}

fn summarize_rubocop(output: &str, failed: bool) -> String {
    if let Some(line) = output.lines().map(str::trim).find(|line| {
        line.contains(" file") && (line.contains(" offense") || line.contains(" no offenses"))
    }) {
        return format!("rubocop: {line}");
    }
    if failed {
        first_nonempty_line(output).unwrap_or_else(|| "rubocop failed".to_string())
    } else {
        first_nonempty_line(output).unwrap_or_else(|| "rubocop completed".to_string())
    }
}

fn summarize_rails_test(output: &str, failed: bool) -> String {
    if let Some(line) = output
        .lines()
        .map(str::trim)
        .find(|line| line.contains(" runs,") && line.contains(" assertions,"))
    {
        return format!("rails test: {line}");
    }
    if failed {
        first_nonempty_line(output).unwrap_or_else(|| "ruby tests failed".to_string())
    } else {
        first_nonempty_line(output).unwrap_or_else(|| "ruby tests completed".to_string())
    }
}

fn compact_ruby_failures(output: &str) -> String {
    output
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && (line.starts_with("Failures:")
                    || line.starts_with(char::is_numeric)
                    || line.contains(".rb:")
                    || line.contains("Offenses:"))
        })
        .take(12)
        .collect::<Vec<_>>()
        .join("\n")
}

fn contains_any(argv: &[String], denied: &[&str]) -> bool {
    argv.iter().any(|arg| {
        denied
            .iter()
            .any(|denied| arg == denied || arg.starts_with(&format!("{denied}=")))
    })
}

fn looks_like_path(value: &str) -> bool {
    value.contains('/') || value.ends_with(".rb")
}

fn first_nonempty_line(value: &str) -> Option<String> {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)
}

fn compact(value: &str, limit: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= limit {
        compact
    } else {
        format!(
            "{}...",
            compact
                .chars()
                .take(limit.saturating_sub(3))
                .collect::<String>()
        )
    }
}

fn fingerprint(family: &str, kind: &str, argv: &[String]) -> String {
    format!("{family}:{kind}:{}", argv.join("\u{1f}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_bundle_exec_rspec_and_summarize_failure() {
        let argv = ["bundle", "exec", "rspec", "spec/models/user_spec.rb"]
            .into_iter()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let spec = classify_ruby_command("bundle exec rspec spec/models/user_spec.rb", &argv)
            .expect("rspec should classify");
        assert_eq!(spec.family, "ruby");
        assert_eq!(spec.canonical_kind, "ruby_rspec");

        let output = "Failures:\n  1) User validates email\n     spec/models/user_spec.rb:12\n\n3 examples, 1 failure\n";
        let reduction = reduce_ruby_command(&spec, output, "", 1);
        assert_eq!(reduction.summary, "rspec: 3 examples, 1 failure");
        assert!(reduction
            .compact_preview
            .contains("spec/models/user_spec.rb:12"));
        assert!(reduction.failed);
    }

    #[test]
    fn classify_rubocop_declines_custom_formats() {
        let argv = ["rubocop", "--format", "json", "app"]
            .into_iter()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        assert!(classify_ruby_command("rubocop --format json app", &argv).is_none());
    }
}
