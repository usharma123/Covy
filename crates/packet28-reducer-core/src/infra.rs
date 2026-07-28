use crate::types::{CommandReducerSpec, CommandReduction};

pub fn classify_infra_command(command: &str, argv: &[String]) -> Option<CommandReducerSpec> {
    let (canonical_kind, operation_kind) = match argv.first()?.as_str() {
        "docker" => match argv.get(1)?.as_str() {
            "compose" => match argv.get(2)?.as_str() {
                "ps" if !contains_any(argv, &["--format", "--quiet", "-q"]) => (
                    "docker_compose_ps",
                    suite_packet_core::ToolOperationKind::Fetch,
                ),
                "logs" if !contains_any(argv, &["--follow", "-f", "--timestamps"]) => (
                    "docker_compose_logs",
                    suite_packet_core::ToolOperationKind::Read,
                ),
                "build" => (
                    "docker_compose_build",
                    suite_packet_core::ToolOperationKind::Build,
                ),
                _ => return None,
            },
            "ps" if !contains_any(argv, &["--format", "--quiet", "-q"]) => {
                ("docker_ps", suite_packet_core::ToolOperationKind::Fetch)
            }
            "images" if !contains_any(argv, &["--format", "--quiet", "-q"]) => {
                ("docker_images", suite_packet_core::ToolOperationKind::Fetch)
            }
            "logs" if !contains_any(argv, &["--follow", "-f", "--timestamps"]) => {
                ("docker_logs", suite_packet_core::ToolOperationKind::Read)
            }
            "build" => ("docker_build", suite_packet_core::ToolOperationKind::Build),
            "run" => ("docker_run", suite_packet_core::ToolOperationKind::Generic),
            "exec" => ("docker_exec", suite_packet_core::ToolOperationKind::Generic),
            _ => return None,
        },
        "kubectl" => match argv.get(1)?.as_str() {
            "get" if !contains_any(argv, &["-o", "--output", "--watch", "-w"]) => {
                ("kubectl_get", suite_packet_core::ToolOperationKind::Fetch)
            }
            "logs" if !contains_any(argv, &["-f", "--follow", "--prefix"]) => {
                ("kubectl_logs", suite_packet_core::ToolOperationKind::Read)
            }
            "describe" => (
                "kubectl_describe",
                suite_packet_core::ToolOperationKind::Read,
            ),
            "apply" => (
                "kubectl_apply",
                suite_packet_core::ToolOperationKind::Generic,
            ),
            _ => return None,
        },
        "curl" if classify_curl(argv) => {
            ("curl_fetch", suite_packet_core::ToolOperationKind::Fetch)
        }
        "wget" if classify_wget(argv) => {
            ("wget_fetch", suite_packet_core::ToolOperationKind::Fetch)
        }
        "aws" => (
            classify_aws_kind(argv)?,
            suite_packet_core::ToolOperationKind::Fetch,
        ),
        "psql" if classify_psql(argv) => {
            ("psql_query", suite_packet_core::ToolOperationKind::Fetch)
        }
        _ => return None,
    };

    let paths = argv
        .iter()
        .skip(1)
        .filter(|arg| !arg.starts_with('-') && looks_like_target(arg))
        .cloned()
        .collect::<Vec<_>>();

    let mutation = matches!(
        canonical_kind,
        "docker_build"
            | "docker_compose_build"
            | "docker_run"
            | "docker_exec"
            | "kubectl_apply"
            | "wget_fetch"
    );
    let cacheable = !mutation;
    Some(CommandReducerSpec {
        family: "infra".to_string(),
        canonical_kind: canonical_kind.to_string(),
        packet_type: "packet28.hook.infra.v2".to_string(),
        operation_kind,
        command: command.to_string(),
        argv: argv.to_vec(),
        cache_fingerprint: fingerprint("infra", canonical_kind, argv),
        cacheable,
        mutation,
        paths,
        equivalence_key: None,
    })
}

pub fn reduce_infra_command(
    spec: &CommandReducerSpec,
    stdout: &str,
    stderr: &str,
    exit_code: i32,
) -> CommandReduction {
    let failed = exit_code != 0;
    let combined = format!("{stdout}\n{stderr}");
    let lines = nonempty_lines(stdout);
    let summary = match spec.canonical_kind.as_str() {
        "docker_ps" => format!("docker ps listed {} container(s)", data_rows(&lines)),
        "docker_images" => format!("docker images listed {} image(s)", data_rows(&lines)),
        "docker_logs" => format!("docker logs returned {} line(s)", lines.len()),
        "docker_build" => summarize_docker_build(&combined, failed),
        "docker_run" => summarize_docker_run_exec("docker run", &combined, failed),
        "docker_exec" => summarize_docker_run_exec("docker exec", &combined, failed),
        "docker_compose_ps" => {
            format!("docker compose ps listed {} service(s)", data_rows(&lines))
        }
        "docker_compose_logs" => format!("docker compose logs returned {} line(s)", lines.len()),
        "docker_compose_build" => summarize_docker_build(&combined, failed),
        "kubectl_get" => summarize_kubectl_get(spec, &lines),
        "kubectl_logs" => format!("kubectl logs returned {} line(s)", lines.len()),
        "kubectl_describe" => summarize_kubectl_describe(stdout),
        "kubectl_apply" => summarize_kubectl_apply(&combined, failed),
        "curl_fetch" => {
            if failed {
                first_nonempty_line(&combined).unwrap_or_else(|| "curl failed".to_string())
            } else {
                summarize_curl(stdout)
            }
        }
        "wget_fetch" => {
            if failed {
                summarize_wget_error(&combined)
            } else {
                summarize_wget(spec, stdout, stderr)
            }
        }
        kind if kind.starts_with("aws_") => {
            if failed {
                first_nonempty_line(&combined).unwrap_or_else(|| "aws command failed".to_string())
            } else {
                summarize_aws(spec, stdout)
            }
        }
        "psql_query" => {
            if failed {
                first_nonempty_line(&combined).unwrap_or_else(|| "psql failed".to_string())
            } else {
                summarize_psql(stdout)
            }
        }
        _ => {
            first_nonempty_line(&combined).unwrap_or_else(|| "infra command completed".to_string())
        }
    };

    CommandReduction {
        family: spec.family.clone(),
        canonical_kind: spec.canonical_kind.clone(),
        packet_type: spec.packet_type.clone(),
        operation_kind: spec.operation_kind,
        summary: if failed && !matches!(spec.canonical_kind.as_str(), "curl_fetch" | "wget_fetch") {
            first_nonempty_line(&combined).unwrap_or(summary)
        } else {
            summary
        },
        compact_preview: match spec.canonical_kind.as_str() {
            "docker_logs" | "docker_compose_logs" | "kubectl_logs" => {
                compact_log_output(stdout, 50)
            }
            "curl_fetch" if !failed => compact_curl_response(stdout),
            "wget_fetch" if !failed => compact_wget_output(spec, stdout, stderr),
            kind if kind.starts_with("aws_") && !failed => compact_aws_output(spec, stdout),
            "psql_query" if !failed => compact_psql_output(stdout),
            _ => String::new(),
        },
        paths: spec.paths.clone(),
        regions: Vec::new(),
        symbols: Vec::new(),
        failed,
        error_class: failed.then(|| "infra_error".to_string()),
        error_message: failed.then(|| compact(&combined, 220)),
        retryable: failed.then_some(false),
        exit_code,
        cache_fingerprint: spec.cache_fingerprint.clone(),
        cacheable: spec.cacheable,
        mutation: spec.mutation,
        equivalence_key: spec.equivalence_key.clone(),
    }
}

fn classify_curl(argv: &[String]) -> bool {
    if contains_any(
        argv,
        &[
            "-o",
            "--output",
            "-O",
            "--remote-name",
            "-w",
            "--write-out",
            "-d",
            "-F",
            "-T",
            "--data",
            "--data-binary",
            "--data-raw",
            "--data-urlencode",
            "--form",
            "--form-string",
            "--head",
            "-I",
            "--json",
            "--upload-file",
        ],
    ) {
        return false;
    }
    if let Some(method) = explicit_curl_method(argv) {
        if method != "GET" && method != "HEAD" {
            return false;
        }
    }
    argv.iter()
        .any(|arg| arg.starts_with("http://") || arg.starts_with("https://"))
}

fn classify_wget(argv: &[String]) -> bool {
    if contains_any(
        argv,
        &[
            "--spider",
            "--mirror",
            "-m",
            "--recursive",
            "-r",
            "--page-requisites",
            "-p",
            "--input-file",
            "-i",
            "--post-data",
            "--post-file",
        ],
    ) {
        return false;
    }
    argv.iter()
        .skip(1)
        .any(|arg| arg.starts_with("http://") || arg.starts_with("https://"))
}

fn classify_psql(argv: &[String]) -> bool {
    if contains_any(
        argv,
        &[
            "-f",
            "--file",
            "-o",
            "--output",
            "--csv",
            "-A",
            "--tuples-only",
            "-t",
        ],
    ) {
        return false;
    }
    argv.len() > 1
}

fn contains_any(argv: &[String], denied: &[&str]) -> bool {
    argv.iter().any(|arg| {
        denied
            .iter()
            .any(|denied| arg == denied || arg.starts_with(&format!("{denied}=")))
    })
}

fn looks_like_target(value: &str) -> bool {
    value.contains('/') || value.contains(':') || value.starts_with("http")
}

fn nonempty_lines(value: &str) -> Vec<String> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn data_rows(lines: &[String]) -> usize {
    lines.len().saturating_sub(1)
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

fn explicit_curl_method(argv: &[String]) -> Option<String> {
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if arg == "-X" || arg == "--request" {
            return iter.next().map(|value| value.to_ascii_uppercase());
        }
        if let Some(method) = arg.strip_prefix("-X") {
            if !method.is_empty() {
                return Some(method.to_ascii_uppercase());
            }
        }
        if let Some(method) = arg.strip_prefix("--request=") {
            return Some(method.to_ascii_uppercase());
        }
    }
    None
}

fn summarize_kubectl_get(spec: &CommandReducerSpec, lines: &[String]) -> String {
    let resource = spec.argv.get(2).map(String::as_str).unwrap_or("resource");
    let rows = data_rows(lines);
    if lines.is_empty() || rows == 0 {
        return format!("kubectl get {resource} returned 0 row(s)");
    }
    let pending_rows = lines
        .iter()
        .skip(1)
        .filter(|line| line.contains(" Pending ") || line.ends_with(" Pending"))
        .map(|line| {
            line.split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect::<Vec<_>>();
    if let Some(first_pending) = pending_rows.first() {
        format!(
            "kubectl get {resource}: {rows} row(s), {} pending; first {first_pending}",
            pending_rows.len()
        )
    } else {
        format!("kubectl get {resource} returned {rows} row(s)")
    }
}

fn summarize_docker_build(output: &str, failed: bool) -> String {
    if failed {
        return first_nonempty_line(output).unwrap_or_else(|| "docker build failed".to_string());
    }
    if let Some(line) = output
        .lines()
        .map(str::trim)
        .rev()
        .find(|line| line.starts_with("Successfully tagged "))
    {
        return compact(line, 180);
    }
    if let Some(line) =
        output.lines().map(str::trim).rev().find(|line| {
            line.starts_with("Successfully built ") || line.starts_with("writing image")
        })
    {
        return compact(line, 180);
    }
    let steps = output
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("Step ") || line.starts_with('#'))
        .count();
    if steps > 0 {
        format!("docker build completed ({steps} build line(s))")
    } else {
        first_nonempty_line(output).unwrap_or_else(|| "docker build completed".to_string())
    }
}

fn summarize_docker_run_exec(label: &str, output: &str, failed: bool) -> String {
    let lines = nonempty_lines(output);
    if failed {
        return first_nonempty_line(output).unwrap_or_else(|| format!("{label} failed"));
    }
    if lines.is_empty() {
        format!("{label} completed with no output")
    } else {
        format!(
            "{label} returned {} line(s); first {}",
            lines.len(),
            compact(&lines[0], 120)
        )
    }
}

fn summarize_kubectl_apply(output: &str, failed: bool) -> String {
    if failed {
        return first_nonempty_line(output).unwrap_or_else(|| "kubectl apply failed".to_string());
    }
    let lines = nonempty_lines(output);
    if lines.is_empty() {
        return "kubectl apply completed with no output".to_string();
    }
    let changed = lines
        .iter()
        .filter(|line| {
            line.ends_with(" created")
                || line.ends_with(" configured")
                || line.ends_with(" unchanged")
                || line.ends_with(" deleted")
        })
        .count();
    if changed > 0 {
        format!(
            "kubectl apply reported {changed} resource line(s); first {}",
            compact(&lines[0], 120)
        )
    } else {
        format!(
            "kubectl apply returned {} line(s); first {}",
            lines.len(),
            compact(&lines[0], 120)
        )
    }
}

fn summarize_curl(stdout: &str) -> String {
    let bytes = stdout.len();
    if let Some(title) = extract_html_title(stdout) {
        format!("curl HTML '{title}' ({bytes}b)")
    } else {
        format!("curl returned {bytes} byte(s)")
    }
}

fn summarize_wget(spec: &CommandReducerSpec, stdout: &str, stderr: &str) -> String {
    let url = wget_url(&spec.argv).unwrap_or("url");
    if wget_outputs_stdout(&spec.argv) {
        let lines = stdout.lines().count();
        return format!(
            "wget {} ok | {} line(s) | {}",
            compact_url(url),
            lines,
            format_size(stdout.len() as u64)
        );
    }
    let filename = wget_filename(&spec.argv, stderr, url);
    let size = wget_saved_size(stderr)
        .map(format_size)
        .unwrap_or_else(|| "?".to_string());
    format!("wget {} ok | {} | {}", compact_url(url), filename, size)
}

fn compact_wget_output(spec: &CommandReducerSpec, stdout: &str, stderr: &str) -> String {
    if wget_outputs_stdout(&spec.argv) {
        let lines = stdout
            .lines()
            .take(10)
            .map(|line| compact(line, 100))
            .collect::<Vec<_>>();
        let total = stdout.lines().count();
        if total > 10 {
            format!("{}\n... +{} more lines", lines.join("\n"), total - 10)
        } else {
            lines.join("\n")
        }
    } else {
        stderr
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                !trimmed.is_empty()
                    && !trimmed.contains("%")
                    && !trimmed.contains("=>")
                    && !trimmed.starts_with("--")
            })
            .take(5)
            .map(|line| compact(line, 120))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn summarize_wget_error(combined: &str) -> String {
    for (needle, label) in [
        ("404", "404 Not Found"),
        ("403", "403 Forbidden"),
        ("401", "401 Unauthorized"),
        ("500", "500 Server Error"),
        ("Connection refused", "Connection refused"),
        ("unable to resolve", "DNS lookup failed"),
        ("Name or service not known", "DNS lookup failed"),
        ("timed out", "Connection timed out"),
        ("certificate", "SSL/TLS error"),
        ("SSL", "SSL/TLS error"),
    ] {
        if combined.contains(needle) {
            return format!("wget failed: {label}");
        }
    }
    first_nonempty_line(combined).unwrap_or_else(|| "wget failed".to_string())
}

fn summarize_kubectl_describe(stdout: &str) -> String {
    let name = stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("Name:"))
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let namespace = stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("Namespace:"))
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let events = stdout
        .lines()
        .position(|line| line.trim() == "Events:")
        .map(|idx| {
            stdout
                .lines()
                .skip(idx + 1)
                .filter(|line| !line.trim().is_empty())
                .count()
        })
        .unwrap_or(0);
    match (name, namespace) {
        (Some(name), Some(namespace)) => {
            format!("kubectl describe: {name} in {namespace} ({events} event line(s))")
        }
        (Some(name), None) => format!("kubectl describe: {name} ({events} event line(s))"),
        _ => "kubectl describe completed".to_string(),
    }
}

fn wget_url(argv: &[String]) -> Option<&str> {
    argv.iter()
        .skip(1)
        .find(|arg| arg.starts_with("http://") || arg.starts_with("https://"))
        .map(String::as_str)
}

fn wget_outputs_stdout(argv: &[String]) -> bool {
    argv.windows(2)
        .any(|window| matches!(window, [flag, value] if (flag == "-O" || flag == "--output-document") && value == "-"))
        || argv.iter().any(|arg| arg == "-O-" || arg == "--output-document=-")
}

fn wget_filename(argv: &[String], stderr: &str, url: &str) -> String {
    for (idx, arg) in argv.iter().enumerate() {
        if (arg == "-O" || arg == "--output-document") && argv.get(idx + 1).is_some() {
            return argv[idx + 1].clone();
        }
        if let Some(name) = arg.strip_prefix("-O") {
            if !name.is_empty() {
                return name.to_string();
            }
        }
        if let Some(name) = arg.strip_prefix("--output-document=") {
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    for line in stderr.lines() {
        if let Some(name) = quoted_after(line, "Saving to:") {
            return name.to_string();
        }
        if let Some(name) = quoted_after(line, "Sauvegarde en") {
            return name.to_string();
        }
    }
    let filename = url
        .split('?')
        .next()
        .unwrap_or(url)
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("index.html");
    if filename.is_empty() || !filename.contains('.') {
        "index.html".to_string()
    } else {
        filename.to_string()
    }
}

fn quoted_after<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let rest = line.split_once(marker)?.1;
    let start = rest.find(['\'', '"', '«'])?;
    let quote = rest[start..].chars().next()?;
    let end_quote = if quote == '«' { '»' } else { quote };
    let content = &rest[start + quote.len_utf8()..];
    let end = content.find(end_quote)?;
    Some(content[..end].trim())
}

fn wget_saved_size(stderr: &str) -> Option<u64> {
    for line in stderr.lines() {
        if !line.contains("saved") && !line.contains("enregistr") {
            continue;
        }
        if let Some((left, _right)) = line
            .split_once('[')
            .and_then(|(_, rest)| rest.split_once('/'))
        {
            let cleaned = left.trim().replace(',', "");
            if let Ok(bytes) = cleaned.parse::<u64>() {
                return Some(bytes);
            }
        }
        for token in line.split_whitespace() {
            let cleaned = token
                .trim_matches(|ch: char| {
                    !ch.is_ascii_digit()
                        && ch != '.'
                        && ch != 'K'
                        && ch != 'M'
                        && ch != 'G'
                        && ch != 'B'
                })
                .trim_end_matches('B');
            if let Some(bytes) = parse_size_token(cleaned) {
                return Some(bytes);
            }
        }
    }
    None
}

fn parse_size_token(value: &str) -> Option<u64> {
    let (number, multiplier) = if let Some(number) = value.strip_suffix('K') {
        (number, 1024.0)
    } else if let Some(number) = value.strip_suffix('M') {
        (number, 1024.0 * 1024.0)
    } else if let Some(number) = value.strip_suffix('G') {
        (number, 1024.0 * 1024.0 * 1024.0)
    } else {
        (value, 1.0)
    };
    let parsed = number.replace(',', "").parse::<f64>().ok()?;
    Some((parsed * multiplier).round() as u64)
}

fn format_size(bytes: u64) -> String {
    if bytes == 0 {
        "?".to_string()
    } else if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn compact_url(url: &str) -> String {
    let without_proto = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    if without_proto.chars().count() <= 50 {
        without_proto.to_string()
    } else {
        let prefix = without_proto.chars().take(25).collect::<String>();
        let suffix = without_proto
            .chars()
            .rev()
            .take(20)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        format!("{prefix}...{suffix}")
    }
}

fn extract_html_title(stdout: &str) -> Option<String> {
    let lower = stdout.to_ascii_lowercase();
    let start = lower.find("<title>")?;
    let end = lower.find("</title>")?;
    if end <= start + 7 {
        return None;
    }
    Some(stdout[start + 7..end].trim().to_string())
}

fn fingerprint(family: &str, kind: &str, argv: &[String]) -> String {
    crate::cache_fingerprint(family, kind, argv)
}

fn classify_aws_kind(argv: &[String]) -> Option<&'static str> {
    if argv.len() < 3 || contains_any(argv, &["--output", "-o"]) {
        return None;
    }
    match (argv.get(1)?.as_str(), argv.get(2)?.as_str()) {
        ("sts", "get-caller-identity") => Some("aws_sts_get_caller_identity"),
        ("s3", "ls") => Some("aws_s3_ls"),
        ("s3api", "list-objects-v2") => Some("aws_s3api_list_objects_v2"),
        ("ec2", "describe-instances") => Some("aws_ec2_describe_instances"),
        ("ec2", "describe-security-groups") => Some("aws_ec2_describe_security_groups"),
        ("ecs", "list-services") => Some("aws_ecs_list_services"),
        ("ecs", "describe-services") => Some("aws_ecs_describe_services"),
        ("ecs", "describe-tasks") => Some("aws_ecs_describe_tasks"),
        ("rds", "describe-db-instances") => Some("aws_rds_describe_db_instances"),
        ("cloudformation", "list-stacks") => Some("aws_cloudformation_list_stacks"),
        ("cloudformation", "describe-stacks") => Some("aws_cloudformation_describe_stacks"),
        ("cloudformation", "describe-stack-events") => {
            Some("aws_cloudformation_describe_stack_events")
        }
        ("logs", "get-log-events") => Some("aws_logs_get_log_events"),
        ("logs", "filter-log-events") => Some("aws_logs_filter_log_events"),
        ("lambda", "list-functions") => Some("aws_lambda_list_functions"),
        ("lambda", "get-function") => Some("aws_lambda_get_function"),
        ("iam", "list-roles") => Some("aws_iam_list_roles"),
        ("iam", "list-users") => Some("aws_iam_list_users"),
        ("dynamodb", "scan") => Some("aws_dynamodb_scan"),
        ("dynamodb", "query") => Some("aws_dynamodb_query"),
        ("dynamodb", "get-item") => Some("aws_dynamodb_get_item"),
        ("eks", "describe-cluster") => Some("aws_eks_describe_cluster"),
        ("sqs", "receive-message") => Some("aws_sqs_receive_message"),
        ("secretsmanager", "list-secrets") => Some("aws_secretsmanager_list_secrets"),
        ("secretsmanager", "get-secret-value") => Some("aws_secretsmanager_get_secret_value"),
        _ => Some("aws_cli"),
    }
}

fn summarize_aws(spec: &CommandReducerSpec, stdout: &str) -> String {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return "aws returned empty response".to_string();
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(summary) = summarize_aws_json(spec.canonical_kind.as_str(), &value) {
            return summary;
        }
        return match value {
            serde_json::Value::Object(map) => {
                let keys: Vec<_> = map.keys().take(5).cloned().collect();
                format!("aws returned object with keys: {}", keys.join(", "))
            }
            serde_json::Value::Array(items) => {
                format!("aws returned {} item(s)", items.len())
            }
            _ => format!(
                "aws returned {}",
                trimmed.chars().take(80).collect::<String>()
            ),
        };
    }
    let lines = nonempty_lines(stdout);
    if spec.canonical_kind == "aws_s3_ls" {
        let prefixes = lines
            .iter()
            .filter(|line| line.split_whitespace().next() == Some("PRE"))
            .count();
        let objects = lines.len().saturating_sub(prefixes);
        format!("aws s3 ls returned {objects} object(s), {prefixes} prefix(es)")
    } else {
        format!("aws returned {} line(s)", lines.len())
    }
}

fn summarize_aws_json(kind: &str, value: &serde_json::Value) -> Option<String> {
    match kind {
        "aws_sts_get_caller_identity" => {
            let account = json_str(value, &["Account"]).unwrap_or("unknown-account");
            let arn = json_str(value, &["Arn"])
                .map(shorten_aws_name)
                .unwrap_or_default();
            Some(if arn.is_empty() {
                format!("aws sts caller account {account}")
            } else {
                format!("aws sts caller account {account} ({arn})")
            })
        }
        "aws_ec2_describe_instances" => {
            let instances = collect_nested_array_items(value, &["Reservations"], "Instances");
            Some(format!(
                "aws ec2 described {} instance(s){}",
                instances.len(),
                first_named_suffix(&instances, &["InstanceId"])
            ))
        }
        "aws_ec2_describe_security_groups" => summarize_named_array(
            "aws ec2 described",
            "security group",
            value,
            &["SecurityGroups"],
            &["GroupName", "GroupId"],
        ),
        "aws_ecs_list_services" => {
            let count = json_array(value, &["serviceArns"]).map_or(0, <[_]>::len);
            Some(format!("aws ecs listed {count} service(s)"))
        }
        "aws_ecs_describe_services" => summarize_named_array(
            "aws ecs described",
            "service",
            value,
            &["services"],
            &["serviceName", "serviceArn"],
        ),
        "aws_ecs_describe_tasks" => summarize_named_array(
            "aws ecs described",
            "task",
            value,
            &["tasks"],
            &["taskArn", "lastStatus"],
        ),
        "aws_rds_describe_db_instances" => summarize_named_array(
            "aws rds described",
            "DB instance",
            value,
            &["DBInstances"],
            &["DBInstanceIdentifier"],
        ),
        "aws_cloudformation_list_stacks" => summarize_named_array(
            "aws cloudformation listed",
            "stack",
            value,
            &["StackSummaries"],
            &["StackName", "StackStatus"],
        ),
        "aws_cloudformation_describe_stacks" => summarize_named_array(
            "aws cloudformation described",
            "stack",
            value,
            &["Stacks"],
            &["StackName", "StackStatus"],
        ),
        "aws_cloudformation_describe_stack_events" => summarize_named_array(
            "aws cloudformation returned",
            "event",
            value,
            &["StackEvents"],
            &["ResourceStatus", "LogicalResourceId"],
        ),
        "aws_logs_get_log_events" | "aws_logs_filter_log_events" => summarize_named_array(
            "aws logs returned",
            "event",
            value,
            &["events"],
            &["message"],
        ),
        "aws_lambda_list_functions" => summarize_named_array(
            "aws lambda listed",
            "function",
            value,
            &["Functions"],
            &["FunctionName", "Runtime"],
        ),
        "aws_lambda_get_function" => {
            let name = json_str(value, &["Configuration", "FunctionName"]).unwrap_or("function");
            let runtime = json_str(value, &["Configuration", "Runtime"]).unwrap_or("unknown");
            Some(format!("aws lambda function {name} ({runtime})"))
        }
        "aws_iam_list_roles" => {
            summarize_named_array("aws iam listed", "role", value, &["Roles"], &["RoleName"])
        }
        "aws_iam_list_users" => {
            summarize_named_array("aws iam listed", "user", value, &["Users"], &["UserName"])
        }
        "aws_s3api_list_objects_v2" => {
            let contents = json_array(value, &["Contents"]).unwrap_or(&[]);
            let total_size = contents
                .iter()
                .filter_map(|item| json_u64(item, &["Size"]))
                .sum::<u64>();
            Some(format!(
                "aws s3api listed {} object(s), {} byte(s)",
                contents.len(),
                total_size
            ))
        }
        "aws_dynamodb_scan" | "aws_dynamodb_query" => {
            let count = json_u64(value, &["Count"]).unwrap_or_else(|| {
                json_array(value, &["Items"])
                    .map(|items| items.len() as u64)
                    .unwrap_or(0)
            });
            Some(format!("aws dynamodb returned {count} item(s)"))
        }
        "aws_dynamodb_get_item" => Some(if value.get("Item").is_some() {
            "aws dynamodb returned item".to_string()
        } else {
            "aws dynamodb returned no item".to_string()
        }),
        "aws_eks_describe_cluster" => {
            let name = json_str(value, &["cluster", "name"]).unwrap_or("cluster");
            let status = json_str(value, &["cluster", "status"]).unwrap_or("unknown");
            Some(format!("aws eks cluster {name} is {status}"))
        }
        "aws_sqs_receive_message" => summarize_named_array(
            "aws sqs received",
            "message",
            value,
            &["Messages"],
            &["MessageId"],
        ),
        "aws_secretsmanager_list_secrets" => summarize_named_array(
            "aws secretsmanager listed",
            "secret",
            value,
            &["SecretList"],
            &["Name", "ARN"],
        ),
        "aws_secretsmanager_get_secret_value" => {
            let name = json_str(value, &["Name"])
                .or_else(|| json_str(value, &["ARN"]).map(shorten_aws_name))
                .unwrap_or("secret");
            let version = json_str(value, &["VersionId"]).unwrap_or("unknown-version");
            Some(format!(
                "aws secretsmanager returned secret {name} ({version})"
            ))
        }
        _ => None,
    }
}

fn compact_aws_output(spec: &CommandReducerSpec, stdout: &str) -> String {
    if spec.canonical_kind == "aws_s3_ls" {
        return nonempty_lines(stdout)
            .into_iter()
            .take(30)
            .collect::<Vec<_>>()
            .join("\n");
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(stdout.trim()) else {
        return nonempty_lines(stdout)
            .into_iter()
            .take(10)
            .collect::<Vec<_>>()
            .join("\n");
    };
    match spec.canonical_kind.as_str() {
        "aws_sts_get_caller_identity" => ["Account", "Arn", "UserId"]
            .iter()
            .filter_map(|key| json_str(&value, &[*key]).map(|v| format!("{key}: {v}")))
            .collect::<Vec<_>>()
            .join("\n"),
        "aws_ec2_describe_instances" => {
            let instances = collect_nested_array_items(&value, &["Reservations"], "Instances");
            compact_json_rows(&instances, &["InstanceId", "InstanceType", "State.Name"])
        }
        "aws_s3api_list_objects_v2" => compact_json_rows(
            json_array(&value, &["Contents"]).unwrap_or(&[]),
            &["Key", "Size", "LastModified"],
        ),
        "aws_lambda_list_functions" => compact_json_rows(
            json_array(&value, &["Functions"]).unwrap_or(&[]),
            &["FunctionName", "Runtime", "LastModified"],
        ),
        "aws_iam_list_roles" => compact_json_rows(
            json_array(&value, &["Roles"]).unwrap_or(&[]),
            &["RoleName", "Arn"],
        ),
        "aws_iam_list_users" => compact_json_rows(
            json_array(&value, &["Users"]).unwrap_or(&[]),
            &["UserName", "Arn"],
        ),
        "aws_logs_get_log_events" | "aws_logs_filter_log_events" => compact_json_rows(
            json_array(&value, &["events"]).unwrap_or(&[]),
            &["timestamp", "message"],
        ),
        "aws_secretsmanager_list_secrets" => compact_json_rows(
            json_array(&value, &["SecretList"]).unwrap_or(&[]),
            &["Name", "ARN", "LastChangedDate"],
        ),
        "aws_secretsmanager_get_secret_value" => {
            ["ARN", "Name", "VersionId", "CreatedDate", "VersionStages"]
                .iter()
                .filter_map(|field| {
                    json_dotted_compact(&value, field).map(|v| format!("{field}: {v}"))
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => compact_curl_response(stdout),
    }
}

fn summarize_named_array(
    prefix: &str,
    singular: &str,
    value: &serde_json::Value,
    path: &[&str],
    first_fields: &[&str],
) -> Option<String> {
    let items = json_array(value, path)?;
    Some(format!(
        "{prefix} {} {}(s){}",
        items.len(),
        singular,
        first_named_suffix(items, first_fields)
    ))
}

fn first_named_suffix(items: &[serde_json::Value], fields: &[&str]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let details = fields
        .iter()
        .filter_map(|field| json_dotted_str(first, field).map(shorten_aws_name))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if details.is_empty() {
        String::new()
    } else {
        format!("; first {}", details.join(" "))
    }
}

fn collect_nested_array_items(
    value: &serde_json::Value,
    outer_path: &[&str],
    inner_key: &str,
) -> Vec<serde_json::Value> {
    json_array(value, outer_path)
        .unwrap_or(&[])
        .iter()
        .flat_map(|item| {
            json_array(item, &[inner_key])
                .unwrap_or(&[])
                .iter()
                .cloned()
        })
        .collect()
}

fn compact_json_rows(items: &[serde_json::Value], fields: &[&str]) -> String {
    let mut rows = Vec::new();
    rows.push(fields.join("\t"));
    for item in items.iter().take(30) {
        rows.push(
            fields
                .iter()
                .map(|field| json_dotted_compact(item, field).unwrap_or_default())
                .collect::<Vec<_>>()
                .join("\t"),
        );
    }
    if items.len() > 30 {
        rows.push(format!("... +{} more rows", items.len() - 30));
    }
    rows.join("\n")
}

fn json_array<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a [serde_json::Value]> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))?
        .as_array()
        .map(Vec::as_slice)
}

fn json_str<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a str> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))?
        .as_str()
}

fn json_u64(value: &serde_json::Value, path: &[&str]) -> Option<u64> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))?
        .as_u64()
}

fn json_dotted_str<'a>(value: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    let mut current = value;
    for part in field.split('.') {
        current = current.get(part)?;
    }
    current.as_str()
}

fn json_dotted_compact(value: &serde_json::Value, field: &str) -> Option<String> {
    let mut current = value;
    for part in field.split('.') {
        current = current.get(part)?;
    }
    match current {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        serde_json::Value::Bool(value) => Some(value.to_string()),
        serde_json::Value::Null => Some(String::new()),
        _ => Some(compact(&current.to_string(), 80)),
    }
}

fn shorten_aws_name(value: &str) -> &str {
    value
        .rsplit(['/', ':'])
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or(value)
}

fn summarize_psql(stdout: &str) -> String {
    if stdout.trim().is_empty() {
        return "psql returned empty output".to_string();
    }
    let compact = compact_psql_output(stdout);
    let rows = compact
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
        .saturating_sub(1);
    if let Some((_, footer_rows)) = parse_psql_footer(stdout) {
        format!("psql returned {footer_rows} row(s)")
    } else if rows > 0 {
        format!("psql returned {rows} row(s)")
    } else {
        first_nonempty_line(stdout).unwrap_or_else(|| "psql completed".to_string())
    }
}

fn compact_psql_output(stdout: &str) -> String {
    let mut rows = Vec::new();
    let mut data_rows = 0usize;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || is_psql_separator(trimmed) || parse_psql_footer(trimmed).is_some()
        {
            continue;
        }
        if trimmed.contains('|') {
            if !rows.is_empty() {
                data_rows += 1;
            }
            if data_rows <= 30 {
                let cols = trimmed
                    .split('|')
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join("\t");
                rows.push(cols);
            }
        } else {
            rows.push(trimmed.to_string());
        }
    }
    if data_rows > 30 {
        rows.push(format!("... +{} more rows", data_rows - 30));
    }
    rows.join("\n")
}

fn is_psql_separator(line: &str) -> bool {
    line.chars().all(|ch| matches!(ch, '-' | '+' | ' ' | '\t'))
        && line.contains('-')
        && line.contains('+')
}

fn parse_psql_footer(line: &str) -> Option<(&str, usize)> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix('(')?.strip_suffix(')')?;
    let count = inner
        .strip_suffix(" rows")
        .or_else(|| inner.strip_suffix(" row"))?
        .trim()
        .parse::<usize>()
        .ok()?;
    Some((trimmed, count))
}

fn compact_log_output(output: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let collapsed = collapse_repeated_log_lines(&lines);
    collapsed
        .into_iter()
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n")
}

fn collapse_repeated_log_lines(lines: &[&str]) -> Vec<String> {
    let mut result = Vec::new();
    let mut prev: Option<&str> = None;
    let mut count = 0usize;
    for &line in lines {
        // Strip timestamp prefix for comparison
        let normalized = strip_log_timestamp(line);
        let prev_normalized = prev.map(strip_log_timestamp);
        if prev_normalized.as_deref() == Some(&normalized) {
            count += 1;
        } else {
            if let Some(prev_line) = prev {
                if count > 1 {
                    result.push(format!("[x{}] {}", count, prev_line));
                } else {
                    result.push(prev_line.to_string());
                }
            }
            prev = Some(line);
            count = 1;
        }
    }
    if let Some(prev_line) = prev {
        if count > 1 {
            result.push(format!("[x{}] {}", count, prev_line));
        } else {
            result.push(prev_line.to_string());
        }
    }
    result
}

fn strip_log_timestamp(line: &str) -> String {
    // Common timestamp patterns: ISO 8601, Docker timestamps, etc.
    // Strip leading YYYY-MM-DD HH:MM:SS or similar
    let chars: Vec<char> = line.chars().collect();
    if chars.len() > 19 {
        let prefix: String = chars[..19].iter().collect();
        if prefix.chars().filter(|c| c.is_ascii_digit()).count() >= 8
            && (prefix.contains('-') || prefix.contains('/'))
            && (prefix.contains(':') || prefix.contains('T'))
        {
            return line[19..].trim().to_string();
        }
    }
    line.to_string()
}

fn compact_curl_response(stdout: &str) -> String {
    // For HTML: extract title
    if let Some(title) = extract_html_title(stdout) {
        return format!("HTML: {title}");
    }
    // For JSON: show structure
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(stdout.trim()) {
        return match value {
            serde_json::Value::Object(map) => {
                let keys: Vec<_> = map.keys().take(10).cloned().collect();
                format!("JSON object with keys: {}", keys.join(", "))
            }
            serde_json::Value::Array(items) => {
                format!("JSON array with {} item(s)", items.len())
            }
            _ => format!(
                "JSON scalar: {}",
                stdout.trim().chars().take(50).collect::<String>()
            ),
        };
    }
    // For plain text: first few lines
    let lines: Vec<&str> = stdout.lines().take(5).collect();
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_infra_declines_following_logs() {
        let argv = vec!["docker", "logs", "-f", "api"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert!(classify_infra_command("docker logs -f api", &argv).is_none());
    }

    #[test]
    fn reduce_kubectl_get_summarizes_rows() {
        let argv = vec!["kubectl", "get", "pods"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_infra_command("kubectl get pods", &argv).unwrap();
        let output = "NAME READY STATUS RESTARTS AGE\napi-123 1/1 Running 0 2d\nworker-456 1/1 Running 1 2d\n";
        let reduction = reduce_infra_command(&spec, output, "", 0);
        assert_eq!(reduction.summary, "kubectl get pods returned 2 row(s)");
    }

    #[test]
    fn reduce_kubectl_get_mentions_pending_rows() {
        let argv = vec!["kubectl", "get", "pods"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_infra_command("kubectl get pods", &argv).unwrap();
        let output =
            "NAME READY STATUS RESTARTS AGE\napi 1/1 Running 0 2d\ncron 0/1 Pending 0 4m\n";
        let reduction = reduce_infra_command(&spec, output, "", 0);
        assert_eq!(
            reduction.summary,
            "kubectl get pods: 2 row(s), 1 pending; first cron"
        );
    }

    #[test]
    fn reduce_curl_extracts_html_title() {
        let argv = vec!["curl", "https://example.com"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_infra_command("curl https://example.com", &argv).unwrap();
        let output = "<html><head><title>Packet28 Fixture</title></head><body></body></html>";
        let reduction = reduce_infra_command(&spec, output, "", 0);
        assert_eq!(reduction.summary, "curl HTML 'Packet28 Fixture' (70b)");
    }

    #[test]
    fn classify_infra_supports_docker_compose_and_kubectl_describe() {
        let argv = vec!["docker", "compose", "ps"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert_eq!(
            classify_infra_command("docker compose ps", &argv)
                .unwrap()
                .canonical_kind,
            "docker_compose_ps"
        );

        let argv = vec!["kubectl", "describe", "pod", "api"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert_eq!(
            classify_infra_command("kubectl describe pod api", &argv)
                .unwrap()
                .canonical_kind,
            "kubectl_describe"
        );
    }

    #[test]
    fn classify_infra_supports_rtk_docker_and_kubectl_mutation_forms() {
        for (command, expected) in [
            ("docker build .", "docker_build"),
            ("docker compose build", "docker_compose_build"),
            ("docker run alpine echo hi", "docker_run"),
            ("docker exec app ls", "docker_exec"),
            ("kubectl apply -f deploy.yaml", "kubectl_apply"),
        ] {
            let argv = shell_words::split(command).unwrap();
            let spec = classify_infra_command(command, &argv).unwrap();
            assert_eq!(spec.canonical_kind, expected, "{command}");
            assert!(spec.mutation, "{command}");
            assert!(!spec.cacheable, "{command}");
        }
    }

    #[test]
    fn classify_infra_declines_mutating_curl_forms() {
        for command in [
            "curl -F file=@payload.txt https://example.com/upload",
            "curl -T payload.txt https://example.com/upload",
            "curl --json '{\"ok\":true}' https://example.com/api",
            "curl --request POST https://example.com/api",
        ] {
            let argv = shell_words::split(command).unwrap();
            assert!(
                classify_infra_command(command, &argv).is_none(),
                "{command}"
            );
        }
    }

    #[test]
    fn classify_wget_fetch_as_uncacheable_mutation() {
        let command = "wget https://example.com/releases/pkg.tar.gz";
        let argv = shell_words::split(command).unwrap();
        let spec = classify_infra_command(command, &argv).unwrap();
        assert_eq!(spec.canonical_kind, "wget_fetch");
        assert!(spec.mutation);
        assert!(!spec.cacheable);
    }

    #[test]
    fn reduce_rtk_docker_and_kubectl_mutation_forms() {
        let argv = vec!["docker", "build", "."]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_infra_command("docker build .", &argv).unwrap();
        let reduction = reduce_infra_command(
            &spec,
            "Step 1/2 : FROM alpine\nSuccessfully built abc123\n",
            "",
            0,
        );
        assert_eq!(reduction.summary, "Successfully built abc123");

        let argv = vec!["docker", "compose", "build"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_infra_command("docker compose build", &argv).unwrap();
        let reduction = reduce_infra_command(
            &spec,
            "#1 [web internal] load build definition\nSuccessfully built compose123\n",
            "",
            0,
        );
        assert_eq!(reduction.summary, "Successfully built compose123");

        let argv = vec!["docker", "exec", "app", "ls"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_infra_command("docker exec app ls", &argv).unwrap();
        let reduction = reduce_infra_command(&spec, "Gemfile\nREADME.md\n", "", 0);
        assert_eq!(
            reduction.summary,
            "docker exec returned 2 line(s); first Gemfile"
        );

        let argv = vec!["kubectl", "apply", "-f", "deploy.yaml"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_infra_command("kubectl apply -f deploy.yaml", &argv).unwrap();
        let reduction = reduce_infra_command(
            &spec,
            "deployment.apps/api configured\nservice/api unchanged\n",
            "",
            0,
        );
        assert_eq!(
            reduction.summary,
            "kubectl apply reported 2 resource line(s); first deployment.apps/api configured"
        );
    }

    #[test]
    fn reduce_kubectl_describe_summarizes_identity_and_events() {
        let argv = vec!["kubectl", "describe", "pod", "api"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_infra_command("kubectl describe pod api", &argv).unwrap();
        let output = "Name:         api\nNamespace:    prod\nStatus:       Running\n\nEvents:\n  Type    Reason   Age\n  Normal  Pulled   2m\n";
        let reduction = reduce_infra_command(&spec, output, "", 0);
        assert_eq!(
            reduction.summary,
            "kubectl describe: api in prod (2 event line(s))"
        );
    }

    #[test]
    fn reduce_psql_table_summarizes_and_compacts_rows() {
        let argv = vec!["psql", "-c", "select id, name from users"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_infra_command("psql -c 'select id, name from users'", &argv).unwrap();
        let output = " id | name \n----+------\n  1 | Ada\n  2 | Grace\n(2 rows)\n";
        let reduction = reduce_infra_command(&spec, output, "", 0);
        assert_eq!(reduction.summary, "psql returned 2 row(s)");
        assert_eq!(reduction.compact_preview, "id\tname\n1\tAda\n2\tGrace");
    }

    #[test]
    fn classify_rtk_psql_connection_forms() {
        for command in ["psql -U postgres -d mydb", "psql postgres://localhost/mydb"] {
            let argv = shell_words::split(command).unwrap();
            let spec = classify_infra_command(command, &argv).unwrap();
            assert_eq!(spec.canonical_kind, "psql_query", "{command}");
            assert!(!spec.mutation, "{command}");
        }

        let argv = shell_words::split("psql -o out.txt -U postgres").unwrap();
        assert!(classify_infra_command("psql -o out.txt -U postgres", &argv).is_none());
    }

    #[test]
    fn reduce_aws_sts_summarizes_identity() {
        let argv = vec!["aws", "sts", "get-caller-identity"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_infra_command("aws sts get-caller-identity", &argv).unwrap();
        assert_eq!(spec.canonical_kind, "aws_sts_get_caller_identity");
        let output = r#"{"UserId":"AIDA123","Account":"123456789012","Arn":"arn:aws:iam::123456789012:user/alice"}"#;
        let reduction = reduce_infra_command(&spec, output, "", 0);
        assert_eq!(
            reduction.summary,
            "aws sts caller account 123456789012 (alice)"
        );
        assert!(reduction.compact_preview.contains("Account: 123456789012"));
        assert!(reduction
            .compact_preview
            .contains("Arn: arn:aws:iam::123456789012:user/alice"));
    }

    #[test]
    fn reduce_aws_ec2_instances_summarizes_nested_reservations() {
        let argv = vec!["aws", "ec2", "describe-instances"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_infra_command("aws ec2 describe-instances", &argv).unwrap();
        assert_eq!(spec.canonical_kind, "aws_ec2_describe_instances");
        let output = r#"{
  "Reservations": [
    {"Instances": [
      {"InstanceId": "i-001", "InstanceType": "t3.micro", "State": {"Name": "running"}},
      {"InstanceId": "i-002", "InstanceType": "m5.large", "State": {"Name": "stopped"}}
    ]}
  ]
}"#;
        let reduction = reduce_infra_command(&spec, output, "", 0);
        assert_eq!(
            reduction.summary,
            "aws ec2 described 2 instance(s); first i-001"
        );
        assert!(reduction
            .compact_preview
            .contains("InstanceId\tInstanceType\tState.Name"));
        assert!(reduction
            .compact_preview
            .contains("i-002\tm5.large\tstopped"));
    }

    #[test]
    fn reduce_aws_s3api_objects_summarizes_size() {
        let argv = vec!["aws", "s3api", "list-objects-v2", "--bucket", "demo"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec =
            classify_infra_command("aws s3api list-objects-v2 --bucket demo", &argv).unwrap();
        assert_eq!(spec.canonical_kind, "aws_s3api_list_objects_v2");
        let output = r#"{"Contents":[{"Key":"a.txt","Size":3},{"Key":"b.log","Size":7}]}"#;
        let reduction = reduce_infra_command(&spec, output, "", 0);
        assert_eq!(
            reduction.summary,
            "aws s3api listed 2 object(s), 10 byte(s)"
        );
        assert!(reduction
            .compact_preview
            .contains("Key\tSize\tLastModified"));
        assert!(reduction.compact_preview.contains("a.txt\t3\t"));
    }

    #[test]
    fn reduce_aws_s3_ls_summarizes_text_output() {
        let argv = vec!["aws", "s3", "ls", "s3://demo"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_infra_command("aws s3 ls s3://demo", &argv).unwrap();
        assert_eq!(spec.canonical_kind, "aws_s3_ls");
        let output =
            "                           PRE logs/\n2026-05-11 10:00:00          12 README.md\n";
        let reduction = reduce_infra_command(&spec, output, "", 0);
        assert_eq!(
            reduction.summary,
            "aws s3 ls returned 1 object(s), 1 prefix(es)"
        );
        assert!(reduction.compact_preview.contains("README.md"));
    }

    #[test]
    fn reduce_aws_secretsmanager_list_summarizes_secret_metadata() {
        let argv = vec!["aws", "secretsmanager", "list-secrets"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_infra_command("aws secretsmanager list-secrets", &argv).unwrap();
        assert_eq!(spec.canonical_kind, "aws_secretsmanager_list_secrets");
        let output = r#"{
  "SecretList": [
    {
      "Name": "prod/db",
      "ARN": "arn:aws:secretsmanager:us-east-1:123456789012:secret:prod/db-AbCd",
      "LastChangedDate": "2026-05-12T10:00:00Z"
    },
    {
      "Name": "prod/api",
      "ARN": "arn:aws:secretsmanager:us-east-1:123456789012:secret:prod/api-EfGh"
    }
  ]
}"#;
        let reduction = reduce_infra_command(&spec, output, "", 0);
        assert_eq!(
            reduction.summary,
            "aws secretsmanager listed 2 secret(s); first db db-AbCd"
        );
        assert!(reduction
            .compact_preview
            .contains("Name\tARN\tLastChangedDate"));
        assert!(reduction.compact_preview.contains("prod/api"));
    }

    #[test]
    fn reduce_aws_secretsmanager_get_secret_value_redacts_secret_payload() {
        let argv = vec![
            "aws",
            "secretsmanager",
            "get-secret-value",
            "--secret-id",
            "prod/db",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        let spec = classify_infra_command(
            "aws secretsmanager get-secret-value --secret-id prod/db",
            &argv,
        )
        .unwrap();
        assert_eq!(spec.canonical_kind, "aws_secretsmanager_get_secret_value");
        let output = r#"{
  "ARN": "arn:aws:secretsmanager:us-east-1:123456789012:secret:prod/db-AbCd",
  "Name": "prod/db",
  "VersionId": "1111-2222",
  "SecretString": "{\"password\":\"super-secret-value\"}",
  "VersionStages": ["AWSCURRENT"],
  "CreatedDate": "2026-05-12T10:00:00Z"
}"#;
        let reduction = reduce_infra_command(&spec, output, "", 0);
        assert_eq!(
            reduction.summary,
            "aws secretsmanager returned secret prod/db (1111-2222)"
        );
        assert!(reduction.compact_preview.contains("Name: prod/db"));
        assert!(reduction.compact_preview.contains("VersionId: 1111-2222"));
        assert!(!reduction.compact_preview.contains("SecretString"));
        assert!(!reduction.compact_preview.contains("super-secret-value"));
    }

    #[test]
    fn reduce_wget_summarizes_saved_file() {
        let argv = vec!["wget", "https://example.com/releases/pkg.tar.gz"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec =
            classify_infra_command("wget https://example.com/releases/pkg.tar.gz", &argv).unwrap();
        assert_eq!(spec.canonical_kind, "wget_fetch");
        let stderr = "--2026-05-12-- https://example.com/releases/pkg.tar.gz\nSaving to: 'pkg.tar.gz'\n'pkg.tar.gz' saved [2048/2048]\n";
        let reduction = reduce_infra_command(&spec, "", stderr, 0);
        assert_eq!(
            reduction.summary,
            "wget example.com/releases/pkg.tar.gz ok | pkg.tar.gz | 2.0KB"
        );
    }

    #[test]
    fn reduce_wget_stdout_compacts_first_lines() {
        let argv = vec!["wget", "-O", "-", "https://example.com/data.txt"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_infra_command("wget -O - https://example.com/data.txt", &argv).unwrap();
        let stdout = (0..12)
            .map(|idx| format!("line {idx}"))
            .collect::<Vec<_>>()
            .join("\n");
        let reduction = reduce_infra_command(&spec, &stdout, "", 0);
        assert_eq!(
            reduction.summary,
            "wget example.com/data.txt ok | 12 line(s) | 85B"
        );
        assert!(reduction.compact_preview.contains("line 0"));
        assert!(reduction.compact_preview.contains("... +2 more lines"));
    }

    #[test]
    fn reduce_wget_error_extracts_http_status() {
        let argv = vec!["wget", "https://example.com/missing.zip"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let spec = classify_infra_command("wget https://example.com/missing.zip", &argv).unwrap();
        let reduction = reduce_infra_command(&spec, "", "ERROR 404: Not Found.\n", 8);
        assert_eq!(reduction.summary, "wget failed: 404 Not Found");
        assert!(reduction.failed);
    }
}
