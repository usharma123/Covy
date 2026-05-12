use crate::types::{CommandReducerSpec, CommandReduction};

pub fn classify_jvm_command(command: &str, argv: &[String]) -> Option<CommandReducerSpec> {
    if argv.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "--stacktrace" | "--info" | "--debug" | "--full-stacktrace"
        )
    }) {
        return None;
    }
    let task = detect_gradle_task(argv);
    let (canonical_kind, mutation) = match task {
        GradleTask::Build => ("jvm_gradle_build", true),
        GradleTask::Test => ("jvm_gradle_test", false),
        GradleTask::ConnectedTest => ("jvm_gradle_connected_test", false),
        GradleTask::Lint => ("jvm_gradle_lint", false),
        GradleTask::Dependencies => ("jvm_gradle_dependencies", false),
        GradleTask::Other => return None,
    };
    let paths = argv
        .iter()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .cloned()
        .collect::<Vec<_>>();
    Some(CommandReducerSpec {
        family: "jvm".to_string(),
        canonical_kind: canonical_kind.to_string(),
        packet_type: "packet28.hook.jvm.v2".to_string(),
        operation_kind: suite_packet_core::ToolOperationKind::Test,
        command: command.to_string(),
        argv: argv.to_vec(),
        cache_fingerprint: fingerprint("jvm", canonical_kind, argv),
        cacheable: !mutation,
        mutation,
        paths,
        equivalence_key: None,
    })
}

pub fn reduce_jvm_command(
    spec: &CommandReducerSpec,
    stdout: &str,
    stderr: &str,
    exit_code: i32,
) -> CommandReduction {
    let failed = exit_code != 0;
    let combined = join_output(stdout, stderr);
    let compact_preview = match spec.canonical_kind.as_str() {
        "jvm_gradle_build" => compact_gradle_build(&combined),
        "jvm_gradle_test" => compact_gradle_test(&combined),
        "jvm_gradle_connected_test" => compact_gradle_connected_test(&combined),
        "jvm_gradle_lint" => compact_gradle_lint(&combined),
        "jvm_gradle_dependencies" => compact_gradle_dependencies(&combined),
        _ => String::new(),
    };
    let summary = match spec.canonical_kind.as_str() {
        "jvm_gradle_build" => summarize_gradle_build(&combined, failed),
        "jvm_gradle_test" => summarize_gradle_test(&combined, failed),
        "jvm_gradle_connected_test" => summarize_gradle_connected_test(&combined, failed),
        "jvm_gradle_lint" => summarize_gradle_lint(&combined, failed),
        "jvm_gradle_dependencies" => summarize_gradle_dependencies(&combined, failed),
        _ => {
            first_nonempty_line(&combined).unwrap_or_else(|| "gradle command completed".to_string())
        }
    };
    CommandReduction {
        family: spec.family.clone(),
        canonical_kind: spec.canonical_kind.clone(),
        packet_type: spec.packet_type.clone(),
        operation_kind: spec.operation_kind,
        summary,
        compact_preview,
        paths: spec.paths.clone(),
        regions: Vec::new(),
        symbols: Vec::new(),
        failed,
        error_class: failed.then(|| "jvm_error".to_string()),
        error_message: failed.then(|| compact(&combined, 200)),
        retryable: failed.then_some(false),
        exit_code,
        cache_fingerprint: spec.cache_fingerprint.clone(),
        cacheable: spec.cacheable,
        mutation: spec.mutation,
        equivalence_key: spec.equivalence_key.clone(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GradleTask {
    Build,
    Test,
    ConnectedTest,
    Lint,
    Dependencies,
    Other,
}

fn detect_gradle_task(argv: &[String]) -> GradleTask {
    let task = argv
        .iter()
        .skip(1)
        .filter(|arg| !arg.starts_with('-') && arg.to_lowercase() != "clean")
        .map(|arg| arg.to_lowercase())
        .next_back()
        .unwrap_or_default();
    if task.contains("connected") {
        GradleTask::ConnectedTest
    } else if task.contains("test") || task == "check" {
        GradleTask::Test
    } else if task.contains("assemble")
        || task.contains("build")
        || task.contains("bundle")
        || task.contains("install")
        || task.is_empty()
    {
        GradleTask::Build
    } else if task.contains("lint") || task.contains("ktlint") || task.contains("detekt") {
        GradleTask::Lint
    } else if task.contains("dependencies") {
        GradleTask::Dependencies
    } else {
        GradleTask::Other
    }
}

fn compact_gradle_build(output: &str) -> String {
    output
        .lines()
        .filter(|line| keep_build_line(line))
        .map(str::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn compact_gradle_test(output: &str) -> String {
    let mut result = Vec::new();
    let mut in_failure_block = false;
    for line in output.lines() {
        if is_task_line(line) || is_try_line(line) {
            continue;
        }
        let trimmed = line.trim();
        if is_build_status(line) || is_actionable(line) || is_test_summary(line) {
            result.push(line.to_string());
            continue;
        }
        if trimmed.ends_with("PASSED") || trimmed.ends_with("SKIPPED") {
            in_failure_block = false;
            continue;
        }
        if trimmed.ends_with("FAILED") || trimmed.contains(" FAILED ") {
            in_failure_block = true;
            result.push(line.to_string());
            continue;
        }
        if in_failure_block {
            if trimmed.starts_with("java.") || trimmed.starts_with("kotlin.") {
                result.push(line.to_string());
            } else if trimmed.starts_with("at ") {
                if !is_framework_frame(trimmed) {
                    result.push(line.to_string());
                    in_failure_block = false;
                }
            } else if !trimmed.is_empty() {
                in_failure_block = false;
            }
        }
    }
    if result.is_empty() && output.contains("BUILD SUCCESSFUL") {
        "ok (no test output; enable Gradle testLogging for details)".to_string()
    } else if result.is_empty() {
        output.trim().to_string()
    } else {
        result.join("\n")
    }
}

fn compact_gradle_connected_test(output: &str) -> String {
    if output.contains("No connected devices!") {
        return "connectedAndroidTest failed: No connected devices! Start an emulator or connect a device.".to_string();
    }
    let stripped = output
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.starts_with("INSTRUMENTATION_STATUS")
                && !trimmed.starts_with("INSTRUMENTATION_RESULT")
                && !trimmed.starts_with("INSTRUMENTATION_CODE")
                && !trimmed.starts_with("Installing APK")
                && !trimmed.starts_with("Starting ")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let compacted = compact_gradle_test(&stripped);
    if compacted.trim().is_empty() {
        "ok (connected tests passed)".to_string()
    } else {
        compacted
    }
}

fn compact_gradle_lint(output: &str) -> String {
    let mut result = Vec::new();
    let mut context_remaining = 0usize;
    for line in output.lines() {
        if is_task_line(line) || is_try_line(line) || is_report_line(line) {
            context_remaining = 0;
            continue;
        }
        let is_android = is_android_lint_line(line);
        if is_build_status(line)
            || is_actionable(line)
            || is_lint_summary(line)
            || is_android
            || is_ktlint_or_detekt_line(line)
        {
            result.push(line.to_string());
            context_remaining = if is_android { 3 } else { 0 };
            continue;
        }
        if context_remaining > 0 {
            if line.trim().is_empty() {
                context_remaining = 0;
            } else {
                result.push(line.to_string());
                context_remaining -= 1;
            }
        }
    }
    if result.is_empty() && output.contains("BUILD SUCCESSFUL") {
        "ok lint passed".to_string()
    } else if result.is_empty() {
        output.trim().to_string()
    } else {
        result.join("\n")
    }
}

fn compact_gradle_dependencies(output: &str) -> String {
    let configs = gradle_dependency_configs(output);
    if configs.is_empty() {
        if output.contains("BUILD SUCCESSFUL") {
            return "ok no dependencies".to_string();
        }
        return output.trim().to_string();
    }
    let total = configs.iter().map(|(_, deps)| deps.len()).sum::<usize>();
    let mut result = format!(
        "{total} top-level dependencies across {} configurations",
        configs.len()
    );
    for (config, deps) in configs {
        result.push_str(&format!("\n\n{config} ({}):", deps.len()));
        for dep in deps.iter().take(20) {
            result.push_str(&format!("\n  {dep}"));
        }
        if deps.len() > 20 {
            result.push_str(&format!("\n  ... +{} more", deps.len() - 20));
        }
    }
    result
}

fn summarize_gradle_build(output: &str, failed: bool) -> String {
    if failed {
        first_matching_line(output, |line| {
            line.starts_with("FAILURE:")
                || line.starts_with("* What went wrong:")
                || line.contains("Execution failed")
                || line.contains("error:")
        })
        .unwrap_or_else(|| "gradle build failed".to_string())
    } else if let Some(status) = first_matching_line(output, is_build_status) {
        status
    } else {
        "gradle build passed".to_string()
    }
}

fn summarize_gradle_test(output: &str, failed: bool) -> String {
    if failed {
        first_matching_line(output, |line| {
            line.contains("tests completed")
                || line.contains("tests failed")
                || line.contains("There were failing tests")
        })
        .or_else(|| first_matching_line(output, |line| line.trim().ends_with("FAILED")))
        .unwrap_or_else(|| "gradle tests failed".to_string())
    } else if let Some(summary) =
        first_matching_line(output, |line| line.contains("tests completed"))
    {
        summary
    } else {
        "gradle tests passed".to_string()
    }
}

fn summarize_gradle_connected_test(output: &str, failed: bool) -> String {
    if output.contains("No connected devices!") {
        "connectedAndroidTest failed: No connected devices".to_string()
    } else if failed {
        summarize_gradle_test(output, true)
    } else {
        "connectedAndroidTest passed".to_string()
    }
}

fn summarize_gradle_lint(output: &str, failed: bool) -> String {
    if failed {
        first_matching_line(output, |line| {
            is_android_lint_line(line) || is_ktlint_or_detekt_line(line)
        })
        .unwrap_or_else(|| "gradle lint failed".to_string())
    } else {
        first_matching_line(output, is_lint_summary)
            .unwrap_or_else(|| "gradle lint passed".to_string())
    }
}

fn summarize_gradle_dependencies(output: &str, failed: bool) -> String {
    if failed {
        first_nonempty_line(output).unwrap_or_else(|| "gradle dependencies failed".to_string())
    } else {
        let configs = gradle_dependency_configs(output);
        let total = configs.iter().map(|(_, deps)| deps.len()).sum::<usize>();
        format!(
            "gradle dependencies: {total} top-level dependencies across {} configurations",
            configs.len()
        )
    }
}

fn gradle_dependency_configs(output: &str) -> Vec<(String, Vec<String>)> {
    let mut configs = Vec::new();
    let mut current_config = String::new();
    let mut current_deps = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || is_task_line(trimmed)
            || is_try_line(trimmed)
            || is_build_status(trimmed)
            || is_actionable(trimmed)
            || trimmed.starts_with("Downloading")
            || trimmed.starts_with("Download ")
            || trimmed.starts_with("Starting a Gradle")
            || trimmed == "No dependencies"
            || trimmed == "(n)"
        {
            continue;
        }
        if !trimmed.starts_with('+')
            && !trimmed.starts_with('|')
            && !trimmed.starts_with('\\')
            && !trimmed.starts_with(' ')
            && trimmed.contains(" - ")
        {
            if !current_config.is_empty() && !current_deps.is_empty() {
                configs.push((current_config.clone(), current_deps.clone()));
            }
            current_config = trimmed.split(" - ").next().unwrap_or(trimmed).to_string();
            current_deps.clear();
            continue;
        }
        if (line.starts_with("+---") || line.starts_with("\\---")) && !current_config.is_empty() {
            current_deps.push(
                trimmed
                    .trim_start_matches("+--- ")
                    .trim_start_matches("\\--- ")
                    .to_string(),
            );
        }
    }
    if !current_config.is_empty() && !current_deps.is_empty() {
        configs.push((current_config, current_deps));
    }
    configs
}

fn keep_build_line(line: &str) -> bool {
    if is_task_line(line) || is_try_line(line) || is_gradle_noise_line(line) {
        return false;
    }
    line.trim().is_empty()
        || is_build_status(line)
        || is_actionable(line)
        || is_build_error_line(line)
        || line.starts_with("w: ")
        || line.starts_with("warning:")
        || line.starts_with("Warning:")
        || line.starts_with("WARNING:")
        || line.contains("gradle.com/s/")
        || line.contains("Publishing build scan")
}

fn is_gradle_noise_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("Starting a Gradle Daemon")
        || trimmed.starts_with("Daemon will be stopped")
        || trimmed.starts_with("Reusing configuration cache")
        || trimmed.starts_with("Calculating task graph")
        || trimmed.starts_with("> Configure project")
        || trimmed.starts_with("Deprecated Gradle features")
        || trimmed.starts_with("You can use")
        || trimmed.starts_with("For more on this")
        || trimmed.starts_with("Configuration cache entry")
        || trimmed.starts_with("Downloading")
        || trimmed.starts_with("Configuring")
        || trimmed.starts_with("Resolving")
        || trimmed.starts_with("[Incubating]")
        || trimmed.starts_with("Wrote HTML report")
        || trimmed.starts_with("[android-")
        || (trimmed.ends_with('%') && trimmed.chars().next().is_some_and(|ch| ch.is_ascii_digit()))
}

fn is_build_error_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    line.starts_with("FAILURE:")
        || line.starts_with("* What went wrong:")
        || line.starts_with("* Where:")
        || line.starts_with("> Could not")
        || line.starts_with("Execution failed")
        || lower.contains("error:")
        || line.starts_with("e: ")
        || line.contains("Lint found ")
}

fn is_framework_frame(trimmed: &str) -> bool {
    trimmed.starts_with("at org.junit.")
        || trimmed.starts_with("at junit.")
        || trimmed.starts_with("at java.lang.reflect.")
        || trimmed.starts_with("at sun.reflect.")
        || trimmed.starts_with("at org.gradle.")
}

fn is_task_line(line: &str) -> bool {
    line.trim_start().starts_with("> Task :")
}

fn is_try_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("* Try:")
        || trimmed.starts_with("> Run with --")
        || trimmed.starts_with("> Get more help at")
}

fn is_build_status(line: &str) -> bool {
    line.starts_with("BUILD SUCCESSFUL") || line.starts_with("BUILD FAILED")
}

fn is_actionable(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(first) = trimmed.split_whitespace().next() else {
        return false;
    };
    first.chars().all(|ch| ch.is_ascii_digit()) && trimmed.contains("actionable task")
}

fn is_test_summary(line: &str) -> bool {
    line.contains("tests completed")
        || line.contains("tests failed")
        || line.contains("There were failing tests")
        || line.contains("See the report at")
}

fn is_android_lint_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    line.contains(':')
        && (lower.contains("error:") || lower.contains("warning:"))
        && line.contains('[')
        && line.contains(']')
}

fn is_ktlint_or_detekt_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let colon_count = line.matches(':').count();
    colon_count >= 3 && (lower.contains("lint") || lower.contains("error"))
}

fn is_lint_summary(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    (lower.contains(" issue") || lower.contains(" error") || lower.contains(" warning"))
        && line.chars().any(|ch| ch.is_ascii_digit())
}

fn is_report_line(line: &str) -> bool {
    line.contains("Wrote HTML report")
        || line.contains("Wrote XML report")
        || line.contains("Wrote text report")
        || line.contains("file://")
        || line.contains("/build/reports/lint")
}

fn first_matching_line(output: &str, predicate: impl Fn(&str) -> bool) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .find(|line| predicate(line))
        .map(ToOwned::to_owned)
}

fn first_nonempty_line(value: &str) -> Option<String> {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)
}

fn join_output(stdout: &str, stderr: &str) -> String {
    match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout.to_string(),
        (true, false) => stderr.to_string(),
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

fn compact(value: &str, limit: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= limit {
        return compact;
    }
    let shortened = compact
        .chars()
        .take(limit.saturating_sub(3))
        .collect::<String>();
    format!("{shortened}...")
}

fn fingerprint(family: &str, kind: &str, argv: &[String]) -> String {
    format!("{family}:{kind}:{}", argv.join("\u{1f}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    #[test]
    fn classify_gradle_tasks_and_declines_verbose_modes() {
        assert_eq!(
            classify_jvm_command(
                "./gradlew assembleDebug",
                &argv(&["./gradlew", "assembleDebug"])
            )
            .unwrap()
            .canonical_kind,
            "jvm_gradle_build"
        );
        assert_eq!(
            classify_jvm_command(
                "gradlew testDebugUnitTest",
                &argv(&["gradlew", "testDebugUnitTest"])
            )
            .unwrap()
            .canonical_kind,
            "jvm_gradle_test"
        );
        assert_eq!(
            classify_jvm_command("gradle :app:lint", &argv(&["gradle", ":app:lint"]))
                .unwrap()
                .canonical_kind,
            "jvm_gradle_lint"
        );
        assert!(classify_jvm_command(
            "gradle build --stacktrace",
            &argv(&["gradle", "build", "--stacktrace"])
        )
        .is_none());
    }

    #[test]
    fn reduce_gradle_build_keeps_status_warnings_and_errors() {
        let spec = classify_jvm_command("./gradlew build", &argv(&["./gradlew", "build"])).unwrap();
        let stdout = "Starting a Gradle Daemon\n> Task :compileKotlin\nw: warning detail\nFAILURE: Build failed with an exception.\n* What went wrong:\nExecution failed for task ':compileKotlin'.\nBUILD FAILED in 2s\n3 actionable tasks: 2 executed, 1 up-to-date\n";
        let reduction = reduce_jvm_command(&spec, stdout, "", 1);
        assert_eq!(
            reduction.summary,
            "FAILURE: Build failed with an exception."
        );
        assert!(reduction.compact_preview.contains("w: warning detail"));
        assert!(!reduction.compact_preview.contains("> Task :compileKotlin"));
    }

    #[test]
    fn reduce_gradle_test_keeps_failure_and_user_frame() {
        let spec = classify_jvm_command("gradle test", &argv(&["gradle", "test"])).unwrap();
        let stdout = "ExampleTest > fails FAILED\n    java.lang.AssertionError: expected true\n        at org.junit.Assert.fail(Assert.java:89)\n        at com.example.ExampleTest.fails(ExampleTest.java:42)\n2 tests completed, 1 failed\nBUILD FAILED in 1s\n";
        let reduction = reduce_jvm_command(&spec, stdout, "", 1);
        assert_eq!(reduction.summary, "2 tests completed, 1 failed");
        assert!(reduction
            .compact_preview
            .contains("ExampleTest > fails FAILED"));
        assert!(reduction.compact_preview.contains("ExampleTest.java:42"));
        assert!(!reduction.compact_preview.contains("org.junit.Assert"));
    }

    #[test]
    fn reduce_gradle_connected_reports_missing_device() {
        let spec = classify_jvm_command(
            "gradle connectedDebugAndroidTest",
            &argv(&["gradle", "connectedDebugAndroidTest"]),
        )
        .unwrap();
        let reduction = reduce_jvm_command(&spec, "No connected devices!\n", "", 1);
        assert_eq!(
            reduction.summary,
            "connectedAndroidTest failed: No connected devices"
        );
        assert!(reduction.compact_preview.contains("No connected devices"));
    }

    #[test]
    fn reduce_gradle_lint_keeps_violation_context() {
        let spec = classify_jvm_command("gradle lint", &argv(&["gradle", "lint"])).unwrap();
        let stdout = "src/main/java/MainActivity.kt:45: Error: Format string invalid [StringFormatInvalid]\n    TextView(context).text = \"%s %s\".format(name)\n                             ~~~~~~~~\n1 errors, 0 warnings\nBUILD FAILED\n";
        let reduction = reduce_jvm_command(&spec, stdout, "", 1);
        assert!(reduction.summary.contains("MainActivity.kt:45"));
        assert!(reduction.compact_preview.contains("TextView"));
        assert!(reduction.compact_preview.contains("1 errors"));
    }

    #[test]
    fn reduce_gradle_dependencies_summarizes_top_level_configs() {
        let spec = classify_jvm_command("gradle dependencies", &argv(&["gradle", "dependencies"]))
            .unwrap();
        let stdout = "compileClasspath - Compile classpath for source set 'main'.\n+--- org.jetbrains.kotlin:kotlin-stdlib:1.9.0\n|    \\--- org.jetbrains:annotations:13.0\n\\--- com.squareup.okio:okio:3.0.0\n\nruntimeClasspath - Runtime classpath.\n+--- ch.qos.logback:logback-classic:1.4.0\nBUILD SUCCESSFUL\n";
        let reduction = reduce_jvm_command(&spec, stdout, "", 0);
        assert_eq!(
            reduction.summary,
            "gradle dependencies: 3 top-level dependencies across 2 configurations"
        );
        assert!(reduction.compact_preview.contains("compileClasspath (2):"));
        assert!(!reduction.compact_preview.contains("annotations:13.0"));
    }
}
