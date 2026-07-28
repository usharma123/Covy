use crate::types::{CommandReducerSpec, CommandReduction};

pub fn classify_dotnet_command(command: &str, argv: &[String]) -> Option<CommandReducerSpec> {
    let (canonical_kind, operation_kind) = match argv.get(1)?.as_str() {
        "test" if !contains_any(argv, &["--logger", "-l", "--list-tests", "-t", "--watch"]) => {
            ("dotnet_test", suite_packet_core::ToolOperationKind::Test)
        }
        "build" if !contains_any(argv, &["--watch"]) => {
            ("dotnet_build", suite_packet_core::ToolOperationKind::Build)
        }
        "restore" => (
            "dotnet_restore",
            suite_packet_core::ToolOperationKind::Fetch,
        ),
        _ => return None,
    };

    Some(CommandReducerSpec {
        family: "dotnet".to_string(),
        canonical_kind: canonical_kind.to_string(),
        packet_type: "packet28.hook.dotnet.v2".to_string(),
        operation_kind,
        command: command.to_string(),
        argv: argv.to_vec(),
        cache_fingerprint: fingerprint("dotnet", canonical_kind, argv),
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

pub fn reduce_dotnet_command(
    spec: &CommandReducerSpec,
    stdout: &str,
    stderr: &str,
    exit_code: i32,
) -> CommandReduction {
    let failed = exit_code != 0;
    let combined = format!("{stdout}\n{stderr}");
    let summary = match spec.canonical_kind.as_str() {
        "dotnet_test" => summarize_dotnet_test(&combined, failed),
        "dotnet_build" => summarize_dotnet_build(&combined, failed),
        "dotnet_restore" => summarize_dotnet_restore(&combined, failed),
        _ => first_nonempty_line(&combined).unwrap_or_else(|| ".NET command completed".to_string()),
    };

    CommandReduction {
        family: spec.family.clone(),
        canonical_kind: spec.canonical_kind.clone(),
        packet_type: spec.packet_type.clone(),
        operation_kind: spec.operation_kind,
        summary,
        compact_preview: if failed {
            compact_dotnet_errors(&combined)
        } else {
            String::new()
        },
        paths: spec.paths.clone(),
        regions: Vec::new(),
        symbols: Vec::new(),
        failed,
        error_class: failed.then(|| "dotnet_error".to_string()),
        error_message: failed.then(|| compact(&combined, 220)),
        retryable: failed.then_some(false),
        exit_code,
        cache_fingerprint: spec.cache_fingerprint.clone(),
        cacheable: spec.cacheable,
        mutation: spec.mutation,
        equivalence_key: spec.equivalence_key.clone(),
    }
}

fn summarize_dotnet_test(output: &str, failed: bool) -> String {
    if let Some(line) = output
        .lines()
        .map(str::trim)
        .find(|line| line.contains("Failed:") && line.contains("Passed:"))
    {
        return format!("dotnet test: {line}");
    }
    if let Some(line) = output
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("Passed!") || line.starts_with("Failed!"))
    {
        return format!("dotnet test: {line}");
    }
    if failed {
        first_nonempty_line(output).unwrap_or_else(|| "dotnet test failed".to_string())
    } else {
        first_nonempty_line(output).unwrap_or_else(|| "dotnet test completed".to_string())
    }
}

fn summarize_dotnet_build(output: &str, failed: bool) -> String {
    if let Some(line) = output
        .lines()
        .map(str::trim)
        .find(|line| matches!(*line, "Build succeeded." | "Build FAILED."))
    {
        if let Some(counts) = output
            .lines()
            .map(str::trim)
            .find(|line| line.contains(" Warning(s)") || line.contains(" Error(s)"))
        {
            return format!("dotnet build: {line} {counts}");
        }
        return format!("dotnet build: {line}");
    }
    if failed {
        first_nonempty_line(output).unwrap_or_else(|| "dotnet build failed".to_string())
    } else {
        first_nonempty_line(output).unwrap_or_else(|| "dotnet build completed".to_string())
    }
}

fn summarize_dotnet_restore(output: &str, failed: bool) -> String {
    if failed {
        first_nonempty_line(output).unwrap_or_else(|| "dotnet restore failed".to_string())
    } else {
        output
            .lines()
            .map(str::trim)
            .find(|line| line.contains("Restored ") || line.contains("Determining projects"))
            .map(|line| format!("dotnet restore: {line}"))
            .unwrap_or_else(|| "dotnet restore completed".to_string())
    }
}

fn compact_dotnet_errors(output: &str) -> String {
    output
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && (line.contains(": error ")
                    || line.contains(" error CS")
                    || line.starts_with("Failed "))
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
    value.contains('/') || value.ends_with(".sln") || value.ends_with(".csproj")
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
    crate::cache_fingerprint(family, kind, argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_dotnet_test_and_summarize_results() {
        let argv = ["dotnet", "test", "Packet28.Tests.csproj"]
            .into_iter()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let spec = classify_dotnet_command("dotnet test Packet28.Tests.csproj", &argv)
            .expect("dotnet test should classify");
        assert_eq!(spec.family, "dotnet");
        assert_eq!(spec.canonical_kind, "dotnet_test");

        let output = "Passed!  - Failed: 0, Passed: 12, Skipped: 0, Total: 12, Duration: 1 s\n";
        let reduction = reduce_dotnet_command(&spec, output, "", 0);
        assert_eq!(
            reduction.summary,
            "dotnet test: Passed!  - Failed: 0, Passed: 12, Skipped: 0, Total: 12, Duration: 1 s"
        );
        assert!(!reduction.failed);
    }

    #[test]
    fn classify_dotnet_test_declines_custom_logger() {
        let argv = ["dotnet", "test", "--logger", "trx"]
            .into_iter()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        assert!(classify_dotnet_command("dotnet test --logger trx", &argv).is_none());
    }
}
