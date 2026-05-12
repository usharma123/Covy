use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{bail, Context, Result};
use clap::{Args, ValueEnum};
use packet28_reducer_core::{
    classify_command_argv, reduce_command_output, SearchRequest, SearchResult,
};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ReadFilterLevel {
    None,
    Minimal,
    Aggressive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum GrepEngine {
    Auto,
    Indexed,
    Legacy,
    Fff,
}

#[derive(Args)]
pub struct ReadArgs {
    /// Files to read. Use - to read stdin.
    #[arg(required = true, num_args = 1..)]
    pub files: Vec<PathBuf>,

    /// Filter level: none, minimal, or aggressive
    #[arg(short, long, value_enum, default_value_t = ReadFilterLevel::None)]
    pub level: ReadFilterLevel,

    /// Maximum lines to show from the start after filtering
    #[arg(short, long, conflicts_with = "tail_lines")]
    pub max_lines: Option<usize>,

    /// Show only the last N lines after filtering
    #[arg(long, conflicts_with = "max_lines")]
    pub tail_lines: Option<usize>,

    /// Show line numbers
    #[arg(short = 'n', long)]
    pub line_numbers: bool,

    /// Emit a machine-readable envelope
    #[arg(long)]
    pub json: bool,

    /// Pretty-print JSON machine output
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct SummaryArgs {
    /// Command to run and summarize
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,

    /// Emit a machine-readable envelope
    #[arg(long)]
    pub json: bool,

    /// Pretty-print JSON machine output
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct FormatterArgs {
    /// Formatter arguments
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,

    /// Emit a machine-readable envelope
    #[arg(long)]
    pub json: bool,

    /// Pretty-print JSON machine output
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct ToolArgs {
    /// Tool arguments
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,

    /// Emit a machine-readable envelope
    #[arg(long)]
    pub json: bool,

    /// Pretty-print JSON machine output
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct SmartArgs {
    /// File to summarize with local heuristics
    pub file: PathBuf,

    /// Compatibility knob for RTK's model flag. Packet28 uses local heuristics.
    #[arg(short, long, default_value = "heuristic")]
    pub model: String,

    /// Compatibility knob for RTK's force-download flag. Packet28 performs no download.
    #[arg(long)]
    pub force_download: bool,

    /// Emit a machine-readable envelope
    #[arg(long)]
    pub json: bool,

    /// Pretty-print JSON machine output
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct FindArgs {
    /// Find arguments. Supports `find <pattern> [path]` and native `find <path> -name <pattern>`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,

    /// Emit a machine-readable envelope
    #[arg(long)]
    pub json: bool,

    /// Pretty-print JSON machine output
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct GrepArgs {
    /// Pattern to search for
    pub pattern: String,

    /// File or directory to search
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Maximum line length to render
    #[arg(short = 'l', long, default_value_t = 80)]
    pub max_len: usize,

    /// Maximum result lines to render
    #[arg(short, long, default_value_t = 200)]
    pub max: usize,

    /// Show a short context slice around the match
    #[arg(long)]
    pub context_only: bool,

    /// Filter by extension, such as rs, py, or ts
    #[arg(short = 't', long)]
    pub file_type: Option<String>,

    /// Search backend to use. `fff` delegates to the existing p28 fff-mcp adapter.
    #[arg(long, value_enum, default_value_t = GrepEngine::Auto)]
    pub engine: GrepEngine,

    /// Emit a machine-readable envelope
    #[arg(long)]
    pub json: bool,

    /// Pretty-print JSON machine output
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct JsonArgs {
    /// JSON file to inspect. Reads stdin when omitted.
    pub file: Option<PathBuf>,

    /// Maximum nesting depth to render before eliding children
    #[arg(long, default_value_t = 5)]
    pub max_depth: usize,

    /// Render type/schema information without scalar values
    #[arg(long, alias = "keys-only")]
    pub schema_only: bool,

    /// Emit a machine-readable envelope
    #[arg(long)]
    pub json: bool,

    /// Pretty-print JSON machine output
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct DepsArgs {
    /// Project directory or file inside a project
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Emit a machine-readable envelope
    #[arg(long)]
    pub json: bool,

    /// Pretty-print JSON machine output
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct EnvArgs {
    /// Only show variables whose names contain this text
    pub filter: Option<String>,

    /// Show sensitive values instead of masking them
    #[arg(long)]
    pub show_all: bool,

    /// Emit a machine-readable envelope
    #[arg(long)]
    pub json: bool,

    /// Pretty-print JSON machine output
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct LogArgs {
    /// Log file to analyze. Reads stdin when omitted.
    pub file: Option<PathBuf>,

    /// Emit a machine-readable envelope
    #[arg(long)]
    pub json: bool,

    /// Pretty-print JSON machine output
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct PipeArgs {
    /// Named filter to apply. Uses local auto-detection when omitted.
    #[arg(short, long)]
    pub filter: Option<String>,

    /// Pass stdin through without filtering
    #[arg(long)]
    pub passthrough: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemOutput {
    command: String,
    summary: String,
    text: String,
}

pub fn run_read(args: ReadArgs) -> Result<i32> {
    let mut rendered_files = Vec::new();
    for file in &args.files {
        let raw = if file.as_os_str() == "-" {
            let mut content = String::new();
            io::stdin()
                .lock()
                .read_to_string(&mut content)
                .context("failed to read from stdin")?;
            content
        } else {
            fs::read_to_string(file)
                .with_context(|| format!("failed to read {}", file.display()))?
        };
        let language = detect_language(file);
        let filtered = filter_read_content(&raw, language, args.level);
        let windowed = apply_line_window(&filtered, args.max_lines, args.tail_lines, language);
        let final_text = if args.line_numbers {
            format_with_line_numbers(&windowed)
        } else {
            windowed
        };
        if args.files.len() > 1 && file.as_os_str() != "-" {
            rendered_files.push(format!("==> {} <==\n{}", file.display(), final_text));
        } else {
            rendered_files.push(final_text);
        }
    }
    let text = rendered_files.join(if args.line_numbers { "\n" } else { "" });
    let summary = summarize_rendered_lines("read", &text);
    emit_system_output(
        args.json,
        args.pretty,
        SystemOutput {
            command: "Packet28 read".to_string(),
            summary,
            text,
        },
    )
}

pub fn run_summary(args: SummaryArgs) -> Result<i32> {
    run_summary_labeled(args, "summary")
}

pub fn run_err(args: SummaryArgs) -> Result<i32> {
    run_summary_labeled(args, "err")
}

pub fn run_prettier(args: FormatterArgs) -> Result<i32> {
    run_formatter_command(
        FormatterInvocation::new("prettier", Vec::new()),
        args,
        "prettier",
    )
}

pub fn run_format(args: FormatterArgs) -> Result<i32> {
    let formatter = detect_formatter_invocation(&args.args)?;
    run_formatter_command(formatter, args, "format")
}

pub fn run_npm(args: ToolArgs) -> Result<i32> {
    run_tool_command("npm", args, "npm")
}

pub fn run_npx(args: ToolArgs) -> Result<i32> {
    run_tool_command("npx", args, "npx")
}

pub fn run_tsc(args: ToolArgs) -> Result<i32> {
    run_tool_command("tsc", args, "tsc")
}

pub fn run_vitest(args: ToolArgs) -> Result<i32> {
    run_tool_command("vitest", args, "vitest")
}

pub fn run_pytest(args: ToolArgs) -> Result<i32> {
    run_tool_command("pytest", args, "pytest")
}

pub fn run_ruff(args: ToolArgs) -> Result<i32> {
    run_tool_command("ruff", args, "ruff")
}

pub fn run_ls(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("ls", args, "ls")
}

pub fn run_tree(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("tree", args, "tree")
}

pub fn run_wc(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("wc", args, "wc")
}

pub fn run_wget(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("wget", args, "wget")
}

pub fn run_curl(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("curl", args, "curl")
}

pub fn run_docker(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("docker", args, "docker")
}

pub fn run_kubectl(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("kubectl", args, "kubectl")
}

pub fn run_gh(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("gh", args, "gh")
}

pub fn run_glab(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("glab", args, "glab")
}

pub fn run_aws(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("aws", args, "aws")
}

pub fn run_psql(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("psql", args, "psql")
}

pub fn run_git(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("git", args, "git")
}

pub fn run_gt(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("gt", args, "gt")
}

pub fn run_cargo(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("cargo", args, "cargo")
}

pub fn run_go(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("go", args, "go")
}

pub fn run_dotnet(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("dotnet", args, "dotnet")
}

pub fn run_golangci_lint(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("golangci-lint", args, "golangci-lint")
}

pub fn run_gradlew(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("gradlew", args, "gradlew")
}

pub fn run_rake(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("rake", args, "rake")
}

pub fn run_rspec(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("rspec", args, "rspec")
}

pub fn run_rubocop(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("rubocop", args, "rubocop")
}

pub fn run_mypy(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("mypy", args, "mypy")
}

pub fn run_pip(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("pip", args, "pip")
}

pub fn run_pnpm(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("pnpm", args, "pnpm")
}

pub fn run_bundle(args: ToolArgs) -> Result<i32> {
    run_reducer_tool_command("bundle", args, "bundle")
}

pub fn run_brew(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("brew", args, "brew")
}

pub fn run_composer(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("composer", args, "composer")
}

pub fn run_df(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("df", args, "df")
}

pub fn run_du(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("du", args, "du")
}

pub fn run_make(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("make", args, "make")
}

pub fn run_mvn(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("mvn", args, "mvn")
}

pub fn run_poetry(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("poetry", args, "poetry")
}

pub fn run_uv(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("uv", args, "uv")
}

pub fn run_terraform(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("terraform", args, "terraform")
}

pub fn run_tofu(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("tofu", args, "tofu")
}

pub fn run_ansible_playbook(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("ansible-playbook", args, "ansible-playbook")
}

pub fn run_fail2ban_client(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("fail2ban-client", args, "fail2ban-client")
}

pub fn run_gcloud(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("gcloud", args, "gcloud")
}

pub fn run_hadolint(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("hadolint", args, "hadolint")
}

pub fn run_helm(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("helm", args, "helm")
}

pub fn run_iptables(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("iptables", args, "iptables")
}

pub fn run_markdownlint(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("markdownlint", args, "markdownlint")
}

pub fn run_mix(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("mix", args, "mix")
}

pub fn run_ping(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("ping", args, "ping")
}

pub fn run_pio(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("pio", args, "pio")
}

pub fn run_pre_commit(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("pre-commit", args, "pre-commit")
}

pub fn run_ps(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("ps", args, "ps")
}

pub fn run_quarto(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("quarto", args, "quarto")
}

pub fn run_rsync(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("rsync", args, "rsync")
}

pub fn run_shellcheck(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("shellcheck", args, "shellcheck")
}

pub fn run_shopify(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("shopify", args, "shopify")
}

pub fn run_sops(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("sops", args, "sops")
}

pub fn run_systemctl(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("systemctl", args, "systemctl")
}

pub fn run_trunk(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("trunk", args, "trunk")
}

pub fn run_yamllint(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("yamllint", args, "yamllint")
}

pub fn run_liquibase(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("liquibase", args, "liquibase")
}

pub fn run_basedpyright(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("basedpyright", args, "basedpyright")
}

pub fn run_biome(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("biome", args, "biome")
}

pub fn run_gcc(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("gcc", args, "gcc")
}

pub fn run_gpp(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("g++", args, "g++")
}

pub fn run_gradle(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("gradle", args, "gradle")
}

pub fn run_jira(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("jira", args, "jira")
}

pub fn run_jj(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("jj", args, "jj")
}

pub fn run_jq(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("jq", args, "jq")
}

pub fn run_just(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("just", args, "just")
}

pub fn run_mise(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("mise", args, "mise")
}

pub fn run_nx(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("nx", args, "nx")
}

pub fn run_ollama(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("ollama", args, "ollama")
}

pub fn run_oxlint(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("oxlint", args, "oxlint")
}

pub fn run_skopeo(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("skopeo", args, "skopeo")
}

pub fn run_ssh(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("ssh", args, "ssh")
}

pub fn run_stat(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("stat", args, "stat")
}

pub fn run_swift(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("swift", args, "swift")
}

pub fn run_task(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("task", args, "task")
}

pub fn run_turbo(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("turbo", args, "turbo")
}

pub fn run_ty(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("ty", args, "ty")
}

pub fn run_xcodebuild(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("xcodebuild", args, "xcodebuild")
}

pub fn run_yadm(args: ToolArgs) -> Result<i32> {
    run_filtered_tool_command("yadm", args, "yadm")
}

fn run_summary_labeled(args: SummaryArgs, label: &str) -> Result<i32> {
    let output = execute_command(&args.command)?;
    let raw = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let exit_code = output.status.code().unwrap_or(1);
    let text = summarize_command_output(&raw, &args.command.join(" "), output.status.success());
    emit_system_output(
        args.json,
        args.pretty,
        SystemOutput {
            command: format!("Packet28 {label}"),
            summary: summarize_rendered_lines(label, &text),
            text,
        },
    )?;
    Ok(exit_code)
}

fn execute_command(command: &[String]) -> Result<Output> {
    let (program, rest) = command
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("command is required"))?;
    Command::new(program)
        .args(rest)
        .output()
        .with_context(|| format!("failed to execute '{}'", command.join(" ")))
}

fn run_tool_command(program: &str, args: ToolArgs, label: &str) -> Result<i32> {
    let mut command = Vec::with_capacity(args.args.len() + 1);
    command.push(program.to_string());
    command.extend(args.args);
    run_summary_labeled(
        SummaryArgs {
            command,
            json: args.json,
            pretty: args.pretty,
        },
        label,
    )
}

fn run_reducer_tool_command(program: &str, args: ToolArgs, label: &str) -> Result<i32> {
    let mut command = Vec::with_capacity(args.args.len() + 1);
    command.push(program.to_string());
    command.extend(args.args);
    let output = execute_command(&command)?;
    let exit_code = output.status.code().unwrap_or(1);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = if let Some(spec) = classify_command_argv(&command.join(" "), &command) {
        let reduction = reduce_command_output(&spec, &stdout, &stderr, exit_code)?;
        if reduction.compact_preview.trim().is_empty() {
            reduction.summary
        } else {
            format!(
                "{}\n{}",
                reduction.summary,
                reduction.compact_preview.trim_end()
            )
        }
    } else {
        summarize_command_output(
            &format!("{stdout}{stderr}"),
            &command.join(" "),
            output.status.success(),
        )
    };
    emit_system_output(
        args.json,
        args.pretty,
        SystemOutput {
            command: format!("Packet28 {label}"),
            summary: summarize_rendered_lines(label, &text),
            text,
        },
    )?;
    Ok(exit_code)
}

fn run_filtered_tool_command(program: &str, args: ToolArgs, label: &str) -> Result<i32> {
    let mut command = Vec::with_capacity(args.args.len() + 1);
    command.push(program.to_string());
    command.extend(args.args);
    let output = execute_command(&command)?;
    let exit_code = output.status.code().unwrap_or(1);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let command_text = command.join(" ");
    let cwd = env::current_dir().context("failed to read current directory")?;
    let text = if let Some(filter) =
        crate::toml_filters::apply_configured_filter(&cwd, &command_text, &stdout, &stderr)?
    {
        filter.output
    } else {
        summarize_command_output(
            &format!("{stdout}{stderr}"),
            &command_text,
            output.status.success(),
        )
    };
    emit_system_output(
        args.json,
        args.pretty,
        SystemOutput {
            command: format!("Packet28 {label}"),
            summary: summarize_rendered_lines(label, &text),
            text,
        },
    )?;
    Ok(exit_code)
}

struct FormatterInvocation {
    program: String,
    prefix_args: Vec<String>,
}

impl FormatterInvocation {
    fn new(program: &str, prefix_args: Vec<String>) -> Self {
        Self {
            program: program.to_string(),
            prefix_args,
        }
    }
}

fn run_formatter_command(
    invocation: FormatterInvocation,
    args: FormatterArgs,
    label: &str,
) -> Result<i32> {
    let mut command = Vec::with_capacity(args.args.len() + 1 + invocation.prefix_args.len());
    command.push(invocation.program.clone());
    command.extend(invocation.prefix_args);
    let rest = if label == "format" && args.args.first().is_some_and(|arg| is_formatter_name(arg)) {
        &args.args[1..]
    } else {
        args.args.as_slice()
    };
    command.extend(rest.iter().cloned());
    run_summary_labeled(
        SummaryArgs {
            command,
            json: args.json,
            pretty: args.pretty,
        },
        label,
    )
}

fn detect_formatter_invocation(args: &[String]) -> Result<FormatterInvocation> {
    if let Some(first) = args.first() {
        if first == "ruff" {
            let prefix = if args.get(1).is_some_and(|arg| arg == "format") {
                Vec::new()
            } else {
                vec!["format".to_string()]
            };
            return Ok(FormatterInvocation::new("ruff", prefix));
        }
        if is_formatter_name(first) {
            return Ok(FormatterInvocation::new(first, Vec::new()));
        }
    }
    if Path::new("package.json").exists()
        || Path::new(".prettierrc").exists()
        || Path::new(".prettierrc.json").exists()
        || Path::new(".prettierrc.js").exists()
    {
        return Ok(FormatterInvocation::new("prettier", Vec::new()));
    }
    if let Ok(pyproject) = fs::read_to_string("pyproject.toml") {
        if pyproject.contains("[tool.ruff") {
            return Ok(FormatterInvocation::new("ruff", vec!["format".to_string()]));
        }
        if pyproject.contains("[tool.black]") {
            return Ok(FormatterInvocation::new("black", Vec::new()));
        }
    }
    Ok(FormatterInvocation::new("prettier", Vec::new()))
}

fn is_formatter_name(value: &str) -> bool {
    matches!(value, "prettier" | "black" | "ruff" | "biome")
}

pub fn run_smart(args: SmartArgs) -> Result<i32> {
    let raw = fs::read_to_string(&args.file)
        .with_context(|| format!("failed to read {}", args.file.display()))?;
    let language = detect_language(&args.file);
    let summary = summarize_source_file(&raw, language);
    let text = format!("{}\n{}", summary.line1, summary.line2);
    emit_system_output(
        args.json,
        args.pretty,
        SystemOutput {
            command: "Packet28 smart".to_string(),
            summary: summarize_rendered_lines("smart", &text),
            text,
        },
    )?;
    Ok(0)
}

pub fn run_find(args: FindArgs) -> Result<i32> {
    let parsed = parse_find_args(&args.args)?;
    let text = render_find_results(&parsed)?;
    emit_system_output(
        args.json,
        args.pretty,
        SystemOutput {
            command: "Packet28 find".to_string(),
            summary: summarize_rendered_lines("find", &text),
            text,
        },
    )
}

pub fn run_grep(args: GrepArgs) -> Result<i32> {
    let text = render_grep_results(&args)?;
    emit_system_output(
        args.json,
        args.pretty,
        SystemOutput {
            command: "Packet28 grep".to_string(),
            summary: summarize_rendered_lines("grep", &text),
            text,
        },
    )
}

pub fn run_json(args: JsonArgs) -> Result<i32> {
    let content = match &args.file {
        Some(path) => {
            validate_json_extension(path)?;
            fs::read_to_string(path)
                .with_context(|| format!("failed to read JSON file {}", path.display()))?
        }
        None => {
            let mut content = String::new();
            io::stdin()
                .lock()
                .read_to_string(&mut content)
                .context("failed to read JSON from stdin")?;
            content
        }
    };

    let text = if args.schema_only {
        filter_json_schema(&content, args.max_depth)?
    } else {
        filter_json_compact(&content, args.max_depth)?
    };
    let summary = summarize_rendered_lines("JSON", &text);
    emit_system_output(
        args.json,
        args.pretty,
        SystemOutput {
            command: "Packet28 json".to_string(),
            summary,
            text,
        },
    )
}

pub fn run_deps(args: DepsArgs) -> Result<i32> {
    let dir = if args.path.is_file() {
        args.path.parent().unwrap_or(Path::new(".")).to_path_buf()
    } else {
        args.path.clone()
    };
    let text = summarize_deps_dir(&dir)?;
    let summary = summarize_rendered_lines("dependencies", &text);
    emit_system_output(
        args.json,
        args.pretty,
        SystemOutput {
            command: "Packet28 deps".to_string(),
            summary,
            text,
        },
    )
}

pub fn run_env(args: EnvArgs) -> Result<i32> {
    let mut vars = env::vars().collect::<Vec<_>>();
    vars.sort_by(|a, b| a.0.cmp(&b.0));
    let text = render_env_vars(&vars, args.filter.as_deref(), args.show_all);
    let summary = summarize_rendered_lines("environment", &text);
    emit_system_output(
        args.json,
        args.pretty,
        SystemOutput {
            command: "Packet28 env".to_string(),
            summary,
            text,
        },
    )
}

pub fn run_log(args: LogArgs) -> Result<i32> {
    let content = match &args.file {
        Some(path) => fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?,
        None => {
            let mut content = String::new();
            io::stdin()
                .lock()
                .read_to_string(&mut content)
                .context("failed to read log input from stdin")?;
            content
        }
    };
    let text = analyze_logs(&content)?;
    let summary = summarize_rendered_lines("logs", &text);
    emit_system_output(
        args.json,
        args.pretty,
        SystemOutput {
            command: "Packet28 log".to_string(),
            summary,
            text,
        },
    )
}

pub fn run_pipe(args: PipeArgs) -> Result<i32> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .context("failed to read pipe input from stdin")?;
    if args.passthrough {
        print!("{input}");
        return Ok(0);
    }
    let output = filter_pipe_input(args.filter.as_deref(), &input)?;
    print!("{output}");
    Ok(0)
}

fn emit_system_output(json_output: bool, pretty: bool, output: SystemOutput) -> Result<i32> {
    if json_output {
        crate::cmd_common::emit_json(&serde_json::to_value(output)?, pretty)?;
    } else {
        print!("{}", output.text);
        if !output.text.ends_with('\n') {
            println!();
        }
    }
    Ok(0)
}

fn summarize_rendered_lines(label: &str, text: &str) -> String {
    format!("{label}: {} lines", text.lines().count())
}

fn validate_json_extension(path: &Path) -> Result<()> {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return Ok(());
    };
    let format_name = match ext {
        "toml" => Some("TOML"),
        "yaml" | "yml" => Some("YAML"),
        "xml" => Some("XML"),
        "csv" => Some("CSV"),
        "ini" => Some("INI"),
        "env" => Some("env"),
        "txt" => Some("plain text"),
        _ => None,
    };
    if let Some(format_name) = format_name {
        let mut message = format!(
            "{} is not a JSON file (detected {}). Use `Packet28 run cat` or `Packet28 deps` for non-JSON files.",
            path.display(),
            format_name
        );
        if ext == "toml" && path.file_name().is_some_and(|name| name == "Cargo.toml") {
            message.push_str(" Tip: use `Packet28 deps` for Cargo.toml.");
        }
        bail!("{message}");
    }
    Ok(())
}

pub fn filter_json_compact(json_str: &str, max_depth: usize) -> Result<String> {
    let value: Value = serde_json::from_str(json_str).context("failed to parse JSON")?;
    Ok(compact_json(&value, 0, max_depth))
}

fn compact_json(value: &Value, depth: usize, max_depth: usize) -> String {
    let indent = "  ".repeat(depth);
    if depth > max_depth {
        return format!("{indent}...");
    }
    match value {
        Value::Null => format!("{indent}null"),
        Value::Bool(value) => format!("{indent}{value}"),
        Value::Number(value) => format!("{indent}{value}"),
        Value::String(value) => {
            if value.chars().count() > 80 {
                let preview = value.chars().take(77).collect::<String>();
                format!("{indent}\"{preview}...\"")
            } else {
                format!("{indent}\"{value}\"")
            }
        }
        Value::Array(values) => compact_json_array(values, depth, max_depth),
        Value::Object(map) => {
            if map.is_empty() {
                return format!("{indent}{{}}");
            }
            let mut lines = vec![format!("{indent}{{")];
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            for (index, key) in keys.iter().enumerate() {
                let value = &map[*key];
                if is_scalar_json(value) {
                    let rendered = compact_json(value, 0, max_depth);
                    lines.push(format!("{indent}  {key}: {}", rendered.trim()));
                } else {
                    lines.push(format!("{indent}  {key}:"));
                    lines.push(compact_json(value, depth + 1, max_depth));
                }
                if index >= 20 {
                    lines.push(format!(
                        "{indent}  ... +{} more keys",
                        keys.len() - index - 1
                    ));
                    break;
                }
            }
            lines.push(format!("{indent}}}"));
            lines.join("\n")
        }
    }
}

fn compact_json_array(values: &[Value], depth: usize, max_depth: usize) -> String {
    let indent = "  ".repeat(depth);
    if values.is_empty() {
        return format!("{indent}[]");
    }
    if values.len() > 5 {
        let first = compact_json(&values[0], depth + 1, max_depth);
        return format!("{indent}[{}, ... +{} more]", first.trim(), values.len() - 1);
    }
    let rendered = values
        .iter()
        .map(|value| compact_json(value, depth + 1, max_depth))
        .collect::<Vec<_>>();
    if values.iter().all(is_scalar_json) {
        let inline = rendered
            .iter()
            .map(|value| value.trim())
            .collect::<Vec<_>>()
            .join(", ");
        format!("{indent}[{inline}]")
    } else {
        let mut lines = vec![format!("{indent}[")];
        for value in rendered {
            lines.push(format!("{value},"));
        }
        lines.push(format!("{indent}]"));
        lines.join("\n")
    }
}

fn is_scalar_json(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

pub fn filter_json_schema(json_str: &str, max_depth: usize) -> Result<String> {
    let value: Value = serde_json::from_str(json_str).context("failed to parse JSON")?;
    Ok(extract_schema(&value, 0, max_depth))
}

fn extract_schema(value: &Value, depth: usize, max_depth: usize) -> String {
    let indent = "  ".repeat(depth);
    if depth > max_depth {
        return format!("{indent}...");
    }
    match value {
        Value::Null => format!("{indent}null"),
        Value::Bool(_) => format!("{indent}bool"),
        Value::Number(number) if number.is_i64() || number.is_u64() => format!("{indent}int"),
        Value::Number(_) => format!("{indent}float"),
        Value::String(value) => {
            if value.starts_with("http://") || value.starts_with("https://") {
                format!("{indent}url")
            } else if value.len() == 10 && value.chars().filter(|ch| *ch == '-').count() == 2 {
                format!("{indent}date?")
            } else if value.is_empty() {
                format!("{indent}string")
            } else {
                format!("{indent}string")
            }
        }
        Value::Array(values) => {
            if values.is_empty() {
                format!("{indent}[]")
            } else {
                let first = extract_schema(&values[0], depth + 1, max_depth);
                if values.len() == 1 {
                    format!("{indent}[\n{first}\n{indent}]")
                } else {
                    format!("{indent}[{}] ({})", first.trim(), values.len())
                }
            }
        }
        Value::Object(map) => {
            if map.is_empty() {
                return format!("{indent}{{}}");
            }
            let mut lines = vec![format!("{indent}{{")];
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            for (index, key) in keys.iter().enumerate() {
                let value = &map[*key];
                let rendered = extract_schema(value, depth + 1, max_depth);
                if is_scalar_json(value) {
                    lines.push(format!("{indent}  {key}: {}", rendered.trim()));
                } else {
                    lines.push(format!("{indent}  {key}:"));
                    lines.push(rendered);
                }
                if index >= 15 {
                    lines.push(format!(
                        "{indent}  ... +{} more keys",
                        keys.len() - index - 1
                    ));
                    break;
                }
            }
            lines.push(format!("{indent}}}"));
            lines.join("\n")
        }
    }
}

fn summarize_deps_dir(dir: &Path) -> Result<String> {
    let mut found = false;
    let mut out = String::new();

    let cargo = dir.join("Cargo.toml");
    if cargo.exists() {
        found = true;
        out.push_str("Rust (Cargo.toml):\n");
        out.push_str(&summarize_cargo(&cargo)?);
    }

    let package = dir.join("package.json");
    if package.exists() {
        found = true;
        out.push_str("Node.js (package.json):\n");
        out.push_str(&summarize_package_json(&package)?);
    }

    let requirements = dir.join("requirements.txt");
    if requirements.exists() {
        found = true;
        out.push_str("Python (requirements.txt):\n");
        out.push_str(&summarize_requirements(&requirements)?);
    }

    let pyproject = dir.join("pyproject.toml");
    if pyproject.exists() {
        found = true;
        out.push_str("Python (pyproject.toml):\n");
        out.push_str(&summarize_pyproject(&pyproject)?);
    }

    let gomod = dir.join("go.mod");
    if gomod.exists() {
        found = true;
        out.push_str("Go (go.mod):\n");
        out.push_str(&summarize_gomod(&gomod)?);
    }

    if !found {
        out.push_str(&format!("No dependency files found in {}\n", dir.display()));
    }
    Ok(out)
}

fn summarize_cargo(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)?;
    let dep_re = Regex::new(r#"^([a-zA-Z0-9_-]+)\s*=\s*(?:"([^"]+)"|.*version\s*=\s*"([^"]+)")"#)?;
    let section_re = Regex::new(r"^\[([^\]]+)\]")?;
    let mut section = String::new();
    let mut deps = Vec::new();
    let mut dev_deps = Vec::new();
    for line in content.lines() {
        if let Some(caps) = section_re.captures(line) {
            section = caps
                .get(1)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
        } else if let Some(caps) = dep_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let version = caps
                .get(2)
                .or(caps.get(3))
                .map(|m| m.as_str())
                .unwrap_or("*");
            match section.as_str() {
                "dependencies" => deps.push(format!("{name} ({version})")),
                "dev-dependencies" => dev_deps.push(format!("{name} ({version})")),
                _ => {}
            }
        }
    }
    Ok(render_dep_sections(
        &[("Dependencies", deps, 10), ("Dev", dev_deps, 5)],
        false,
    ))
}

fn summarize_package_json(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&content)?;
    let mut out = String::new();
    if let Some(name) = value.get("name").and_then(Value::as_str) {
        let version = value.get("version").and_then(Value::as_str).unwrap_or("?");
        out.push_str(&format!("  {name} @ {version}\n"));
    }
    if let Some(deps) = value.get("dependencies").and_then(Value::as_object) {
        let items = deps
            .iter()
            .map(|(name, version)| format!("{} ({})", name, version.as_str().unwrap_or("*")))
            .collect::<Vec<_>>();
        out.push_str(&render_dep_section("Dependencies", &items, 10, true));
    }
    if let Some(dev_deps) = value.get("devDependencies").and_then(Value::as_object) {
        let items = dev_deps.keys().cloned().collect::<Vec<_>>();
        out.push_str(&render_dep_section("Dev Dependencies", &items, 5, true));
    }
    Ok(out)
}

fn summarize_requirements(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)?;
    let dep_re = Regex::new(r"^([a-zA-Z0-9_-]+)([=<>!~]+.*)?$")?;
    let deps = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let caps = dep_re.captures(line)?;
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            Some(format!("{name}{version}"))
        })
        .collect::<Vec<_>>();
    Ok(render_dep_section("Packages", &deps, 15, true))
}

fn summarize_pyproject(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)?;
    let mut deps = Vec::new();
    let mut in_deps = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("dependencies") && trimmed.contains('[') {
            in_deps = true;
            continue;
        }
        if in_deps {
            if trimmed == "]" {
                break;
            }
            let dep = trimmed.trim_matches(|ch| ch == '"' || ch == '\'' || ch == ',');
            if !dep.is_empty() {
                deps.push(dep.to_string());
            }
        }
    }
    Ok(render_dep_section("Dependencies", &deps, 10, false))
}

fn summarize_gomod(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)?;
    let mut module = String::new();
    let mut go_version = String::new();
    let mut deps = Vec::new();
    let mut in_require = false;
    for line in content.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix("module ") {
            module = name.to_string();
        } else if let Some(version) = line.strip_prefix("go ") {
            go_version = version.to_string();
        } else if line == "require (" {
            in_require = true;
        } else if line == ")" {
            in_require = false;
        } else if in_require && !line.starts_with("//") {
            let parts = line.split_whitespace().collect::<Vec<_>>();
            if parts.len() >= 2 {
                deps.push(format!("{} {}", parts[0], parts[1]));
            }
        } else if let Some(dep) = line.strip_prefix("require ") {
            if !dep.contains('(') {
                deps.push(dep.to_string());
            }
        }
    }
    let mut out = String::new();
    if !module.is_empty() {
        out.push_str(&format!("  {module} (go {go_version})\n"));
    }
    out.push_str(&render_dep_section("Dependencies", &deps, 10, false));
    Ok(out)
}

fn render_dep_sections(sections: &[(&str, Vec<String>, usize)], blank_between: bool) -> String {
    let mut out = String::new();
    for (index, (label, items, limit)) in sections.iter().enumerate() {
        if blank_between && index > 0 && !items.is_empty() {
            out.push('\n');
        }
        out.push_str(&render_dep_section(label, items, *limit, false));
    }
    out
}

fn render_dep_section(label: &str, items: &[String], limit: usize, include_empty: bool) -> String {
    if items.is_empty() && !include_empty {
        return String::new();
    }
    let mut out = format!("  {label} ({}):\n", items.len());
    for item in items.iter().take(limit) {
        out.push_str(&format!("    {item}\n"));
    }
    if items.len() > limit {
        out.push_str(&format!("    ... +{} more\n", items.len() - limit));
    }
    out
}

fn summarize_command_output(output: &str, command: &str, success: bool) -> String {
    let lines = output.lines().collect::<Vec<_>>();
    let mut rendered = vec![
        format!(
            "{} Command: {}",
            if success { "[ok]" } else { "[FAIL]" },
            compact_for_log(command, 80)
        ),
        format!("   {} lines of output", lines.len()),
        String::new(),
    ];
    match detect_summary_output_type(output, command) {
        SummaryOutputType::Test => summarize_tests_for_command(output, &mut rendered),
        SummaryOutputType::Build => summarize_build_for_command(output, &mut rendered),
        SummaryOutputType::Json => summarize_json_for_command(output, &mut rendered),
        SummaryOutputType::Log => {
            let logs = analyze_logs(output).unwrap_or_else(|_| output.to_string());
            rendered.push(logs);
        }
        SummaryOutputType::List => summarize_list_for_command(output, &mut rendered),
        SummaryOutputType::Generic => summarize_generic_for_command(output, &mut rendered),
    }
    rendered.join("\n")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SummaryOutputType {
    Test,
    Build,
    Json,
    Log,
    List,
    Generic,
}

fn detect_summary_output_type(output: &str, command: &str) -> SummaryOutputType {
    let command = command.to_lowercase();
    let lower = output.to_lowercase();
    let trimmed = output.trim_start();
    if command.contains("test") || lower.contains("passed") && lower.contains("failed") {
        SummaryOutputType::Test
    } else if command.contains("build")
        || command.contains("compile")
        || lower.contains("compiling")
    {
        SummaryOutputType::Build
    } else if trimmed.starts_with('{') || trimmed.starts_with('[') {
        SummaryOutputType::Json
    } else if lower.contains("error:")
        || lower.contains("warn:")
        || lower.contains("fatal")
        || lower.contains("panic")
    {
        SummaryOutputType::Log
    } else if output
        .lines()
        .all(|line| line.len() < 200 && line.split_whitespace().count() < 10)
    {
        SummaryOutputType::List
    } else {
        SummaryOutputType::Generic
    }
}

fn summarize_tests_for_command(output: &str, rendered: &mut Vec<String>) {
    let lower = output.to_lowercase();
    rendered.push("Test Results:".to_string());
    let passed = extract_labeled_number(&lower, "passed").unwrap_or(0);
    let failed = extract_labeled_number(&lower, "failed").unwrap_or(0);
    let skipped = extract_labeled_number(&lower, "skipped")
        .or_else(|| extract_labeled_number(&lower, "ignored"));
    rendered.push(format!("   [ok] {passed} passed"));
    if failed > 0 {
        rendered.push(format!("   [FAIL] {failed} failed"));
    }
    if let Some(skipped) = skipped {
        rendered.push(format!("   skip {skipped} skipped"));
    }
    for line in output
        .lines()
        .filter(|line| {
            let lower = line.to_lowercase();
            (lower.contains("failed") || lower.contains("failure")) && !lower.contains("0 failed")
        })
        .take(5)
    {
        rendered.push(format!("   {}", compact_for_log(line, 90)));
    }
}

fn summarize_build_for_command(output: &str, rendered: &mut Vec<String>) {
    let mut errors = 0usize;
    let mut warnings = 0usize;
    let mut compiled = 0usize;
    let mut examples = Vec::new();
    for line in output.lines() {
        let lower = line.to_lowercase();
        if lower.contains("error") && !lower.contains("0 error") {
            errors += 1;
            if examples.len() < 5 {
                examples.push(line);
            }
        } else if lower.contains("warning") && !lower.contains("0 warning") {
            warnings += 1;
        } else if lower.contains("compiling") || lower.contains("compiled") {
            compiled += 1;
        }
    }
    rendered.push("Build Summary:".to_string());
    if compiled > 0 {
        rendered.push(format!("   {compiled} crates/files compiled"));
    }
    rendered.push(format!("   [error] {errors} errors"));
    rendered.push(format!("   [warn] {warnings} warnings"));
    for example in examples {
        rendered.push(format!("   {}", compact_for_log(example, 90)));
    }
}

fn summarize_json_for_command(output: &str, rendered: &mut Vec<String>) {
    rendered.push("JSON Summary:".to_string());
    match filter_json_schema(output, 3) {
        Ok(schema) => rendered.push(schema),
        Err(_) => summarize_generic_for_command(output, rendered),
    }
}

fn summarize_list_for_command(output: &str, rendered: &mut Vec<String>) {
    let lines = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    rendered.push(format!("List Output: {} entries", lines.len()));
    for line in lines.iter().take(10) {
        rendered.push(format!("   {}", compact_for_log(line, 100)));
    }
    if lines.len() > 10 {
        rendered.push(format!("   ... +{} more", lines.len() - 10));
    }
}

fn summarize_generic_for_command(output: &str, rendered: &mut Vec<String>) {
    rendered.push("Output Preview:".to_string());
    for line in output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(10)
    {
        rendered.push(format!("   {}", compact_for_log(line, 100)));
    }
}

fn extract_labeled_number(value: &str, label: &str) -> Option<usize> {
    value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .collect::<Vec<_>>()
        .windows(2)
        .find_map(|pair| {
            if pair.get(1).copied() == Some(label) {
                pair.first()?.parse().ok()
            } else {
                None
            }
        })
}

#[derive(Debug, Clone)]
struct ParsedFindArgs {
    pattern: String,
    path: PathBuf,
    max_results: usize,
    max_depth: Option<usize>,
    file_type: String,
    case_insensitive: bool,
}

impl Default for ParsedFindArgs {
    fn default() -> Self {
        Self {
            pattern: "*".to_string(),
            path: PathBuf::from("."),
            max_results: 50,
            max_depth: None,
            file_type: "f".to_string(),
            case_insensitive: false,
        }
    }
}

fn parse_find_args(args: &[String]) -> Result<ParsedFindArgs> {
    const UNSUPPORTED: &[&str] = &[
        "-not", "!", "-or", "-o", "-and", "-a", "-exec", "-execdir", "-delete", "-print0",
        "-newer", "-perm", "-size", "-mtime", "-mmin", "-atime", "-amin", "-ctime", "-cmin",
        "-empty", "-link", "-regex", "-iregex",
    ];
    if args.iter().any(|arg| UNSUPPORTED.contains(&arg.as_str())) {
        bail!("Packet28 find does not support compound predicates or actions; use native find directly");
    }
    if args.is_empty() {
        return Ok(ParsedFindArgs::default());
    }
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-name" | "-iname" | "-type" | "-maxdepth"))
    {
        parse_native_find_args(args)
    } else {
        parse_compact_find_args(args)
    }
}

fn parse_native_find_args(args: &[String]) -> Result<ParsedFindArgs> {
    let mut parsed = ParsedFindArgs::default();
    let mut idx = 0usize;
    if let Some(first) = args.first() {
        if !first.starts_with('-') {
            parsed.path = PathBuf::from(first);
            idx = 1;
        }
    }
    while idx < args.len() {
        match args[idx].as_str() {
            "-name" => {
                idx += 1;
                parsed.pattern = args.get(idx).cloned().context("missing -name value")?;
            }
            "-iname" => {
                idx += 1;
                parsed.pattern = args.get(idx).cloned().context("missing -iname value")?;
                parsed.case_insensitive = true;
            }
            "-type" => {
                idx += 1;
                parsed.file_type = args.get(idx).cloned().context("missing -type value")?;
            }
            "-maxdepth" => {
                idx += 1;
                parsed.max_depth = Some(
                    args.get(idx)
                        .context("missing -maxdepth value")?
                        .parse()
                        .context("invalid -maxdepth value")?,
                );
            }
            _ => {}
        }
        idx += 1;
    }
    Ok(parsed)
}

fn parse_compact_find_args(args: &[String]) -> Result<ParsedFindArgs> {
    let mut parsed = ParsedFindArgs {
        pattern: args.first().cloned().unwrap_or_else(|| "*".to_string()),
        ..ParsedFindArgs::default()
    };
    let mut idx = 1usize;
    if let Some(path) = args.get(idx) {
        if !path.starts_with('-') {
            parsed.path = PathBuf::from(path);
            idx += 1;
        }
    }
    while idx < args.len() {
        match args[idx].as_str() {
            "-m" | "--max" => {
                idx += 1;
                parsed.max_results = args
                    .get(idx)
                    .context("missing --max value")?
                    .parse()
                    .context("invalid --max value")?;
            }
            "-t" | "--file-type" => {
                idx += 1;
                parsed.file_type = args
                    .get(idx)
                    .cloned()
                    .context("missing --file-type value")?;
            }
            _ => {}
        }
        idx += 1;
    }
    Ok(parsed)
}

fn render_find_results(args: &ParsedFindArgs) -> Result<String> {
    let mut matches = Vec::new();
    let include_hidden = args.pattern.starts_with('.');
    visit_find_path(
        &args.path,
        0,
        args.max_depth,
        include_hidden,
        args,
        &mut matches,
    )?;
    matches.sort();
    let total = matches.len();
    let shown = total.min(args.max_results);
    let mut out = format!(
        "{} match(es) under {} for {}\n",
        total,
        args.path.display(),
        args.pattern
    );
    for path in matches.iter().take(args.max_results) {
        out.push_str(&format!("{}\n", path.display()));
    }
    if total > shown {
        out.push_str(&format!("[+{} more]\n", total - shown));
    }
    Ok(out)
}

fn visit_find_path(
    path: &Path,
    depth: usize,
    max_depth: Option<usize>,
    include_hidden: bool,
    args: &ParsedFindArgs,
    matches: &mut Vec<PathBuf>,
) -> Result<()> {
    if max_depth.is_some_and(|max| depth > max) {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !include_hidden && depth > 0 && name.starts_with('.') {
        return Ok(());
    }
    let is_dir = metadata.is_dir();
    let type_matches = match args.file_type.as_str() {
        "d" => is_dir,
        "f" => metadata.is_file(),
        _ => true,
    };
    if type_matches && glob_name_matches(&args.pattern, name, args.case_insensitive) {
        matches.push(path.to_path_buf());
    }
    if is_dir {
        for entry in
            fs::read_dir(path).with_context(|| format!("failed to read {}", path.display()))?
        {
            let entry = entry?;
            visit_find_path(
                &entry.path(),
                depth + 1,
                max_depth,
                include_hidden,
                args,
                matches,
            )?;
        }
    }
    Ok(())
}

fn glob_name_matches(pattern: &str, name: &str, case_insensitive: bool) -> bool {
    let pattern = if pattern == "." { "*" } else { pattern };
    if case_insensitive {
        glob_match(&pattern.to_lowercase(), &name.to_lowercase())
    } else {
        glob_match(pattern, name)
    }
}

fn glob_match(pattern: &str, name: &str) -> bool {
    glob_match_inner(pattern.as_bytes(), name.as_bytes())
}

fn glob_match_inner(pattern: &[u8], name: &[u8]) -> bool {
    match (pattern.first(), name.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            glob_match_inner(&pattern[1..], name)
                || (!name.is_empty() && glob_match_inner(pattern, &name[1..]))
        }
        (Some(b'?'), Some(_)) => glob_match_inner(&pattern[1..], &name[1..]),
        (Some(pattern_byte), Some(name_byte)) if pattern_byte == name_byte => {
            glob_match_inner(&pattern[1..], &name[1..])
        }
        _ => false,
    }
}

fn render_grep_results(args: &GrepArgs) -> Result<String> {
    let pattern = args.pattern.replace(r"\|", "|");
    let regex =
        Regex::new(&pattern).with_context(|| format!("invalid grep pattern '{pattern}'"))?;
    let mut result = search_result_for_grep(args)?;
    filter_search_result_by_file_type(&mut result, args.file_type.as_deref());
    render_search_result_for_grep(&result, args, &regex)
}

fn search_result_for_grep(args: &GrepArgs) -> Result<SearchResult> {
    if matches!(args.engine, GrepEngine::Fff | GrepEngine::Indexed) {
        return p28_search_result(args).or_else(|err| {
            if args.engine == GrepEngine::Fff {
                Err(err)
            } else {
                reducer_search_result(args)
            }
        });
    }
    reducer_search_result(args)
}

fn reducer_search_result(args: &GrepArgs) -> Result<SearchResult> {
    let (root, requested_paths) = grep_root_and_paths(&args.path)?;
    packet28_reducer_core::search(
        &root,
        &SearchRequest {
            query: args.pattern.clone(),
            requested_paths,
            max_matches_per_file: Some(1000),
            max_total_matches: Some(1000),
            ..SearchRequest::default()
        },
    )
}

fn p28_search_result(args: &GrepArgs) -> Result<SearchResult> {
    let bin = p28_binary().context("p28 search binary was not found")?;
    let (root, requested_paths) = grep_root_and_paths(&args.path)?;
    let mut command = Command::new(&bin);
    command.current_dir(&root);
    command.arg(&args.pattern);
    for path in requested_paths {
        command.arg(path);
    }
    command
        .args(["--json", "--engine", grep_engine_arg(args.engine)])
        .arg("--max-total-matches")
        .arg("1000")
        .arg("--max-matches-per-file")
        .arg("1000");
    let output = command
        .output()
        .with_context(|| format!("failed to execute p28 search backend '{}'", bin.display()))?;
    if !output.status.success() {
        bail!(
            "p28 search backend failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let value: Value =
        serde_json::from_slice(&output.stdout).context("p28 emitted invalid JSON")?;
    serde_json::from_value(value["result"].clone()).context("p28 JSON missing search result")
}

fn grep_root_and_paths(path: &Path) -> Result<(PathBuf, Vec<String>)> {
    if path.is_absolute() {
        if path.is_file() {
            let root = path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("absolute file path has no parent"))?
                .to_path_buf();
            let file = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| anyhow::anyhow!("absolute file path is not valid UTF-8"))?
                .to_string();
            return Ok((root, vec![file]));
        }
        return Ok((path.to_path_buf(), Vec::new()));
    }
    let root = env::current_dir().context("failed to resolve current directory")?;
    let value = path.to_string_lossy().replace('\\', "/");
    let requested_paths = (value != ".").then_some(value).into_iter().collect();
    Ok((root, requested_paths))
}

fn grep_engine_arg(engine: GrepEngine) -> &'static str {
    match engine {
        GrepEngine::Auto => "auto",
        GrepEngine::Indexed => "indexed",
        GrepEngine::Legacy => "legacy",
        GrepEngine::Fff => "fff",
    }
}

fn p28_binary() -> Option<PathBuf> {
    if let Some(bin) = env::var_os("P28_SEARCH_BIN").map(PathBuf::from) {
        if bin.is_file() {
            return Some(bin);
        }
    }
    if let Ok(current) = env::current_exe() {
        if let Some(parent) = current.parent() {
            let sibling = parent.join("p28");
            if sibling.is_file() {
                return Some(sibling);
            }
        }
    }
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|path| path.join("p28"))
            .find(|path| path.is_file())
    })
}

fn filter_search_result_by_file_type(result: &mut SearchResult, file_type: Option<&str>) {
    let Some(file_type) = file_type.map(|value| value.trim_start_matches('.')) else {
        return;
    };
    result.groups.retain_mut(|group| {
        if !Path::new(&group.path)
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext == file_type)
        {
            return false;
        }
        group.matches.retain(|item| {
            Path::new(&item.path)
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == file_type)
        });
        group.match_count = group.matches.len();
        group.displayed_match_count = group.matches.len();
        !group.matches.is_empty()
    });
    result.match_count = result.groups.iter().map(|group| group.match_count).sum();
    result.returned_match_count = result
        .groups
        .iter()
        .map(|group| group.matches.len())
        .sum::<usize>();
    result.paths = result
        .groups
        .iter()
        .map(|group| group.path.clone())
        .collect();
    result.resolved_paths = result.paths.clone();
    result.regions = result
        .groups
        .iter()
        .flat_map(|group| {
            group
                .matches
                .iter()
                .map(|item| format!("{}:{}-{}", item.path, item.line, item.line))
        })
        .collect();
}

fn render_search_result_for_grep(
    result: &SearchResult,
    args: &GrepArgs,
    regex: &Regex,
) -> Result<String> {
    let mut matches = result
        .groups
        .iter()
        .flat_map(|group| group.matches.iter())
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.line.cmp(&b.line)));
    if matches.is_empty() {
        return Ok(format!("0 matches for '{}'\n", args.pattern));
    }
    let file_count = result.paths.len();
    let total = result.match_count;
    let mut out = format!("{total} matches in {file_count} files:\n\n");
    for item in matches.iter().take(args.max) {
        out.push_str(&format!(
            "{}:{}:{}\n",
            compact_path_for_grep(Path::new(&item.path)),
            item.line,
            clean_grep_line(&item.text, args.max_len, args.context_only, &regex)
        ));
    }
    if total > args.max {
        out.push_str(&format!("[+{} more]\n", total - args.max));
    }
    Ok(out)
}

fn clean_grep_line(line: &str, max_len: usize, context_only: bool, regex: &Regex) -> String {
    let trimmed = line.trim();
    if context_only {
        if let Some(mat) = regex.find(trimmed) {
            let chars = trimmed.chars().collect::<Vec<_>>();
            let start = trimmed[..mat.start()].chars().count().saturating_sub(20);
            let end = (trimmed[..mat.end()].chars().count() + 20).min(chars.len());
            return chars[start..end].iter().collect::<String>();
        }
    }
    compact_for_log(trimmed, max_len)
}

fn compact_path_for_grep(path: &Path) -> String {
    let display = path.display().to_string();
    if display.len() <= 50 {
        return display;
    }
    let parts = display.split('/').collect::<Vec<_>>();
    if parts.len() <= 3 {
        display
    } else {
        format!(
            "{}/.../{}/{}",
            parts[0],
            parts[parts.len() - 2],
            parts[parts.len() - 1]
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadLanguage {
    Rust,
    JavaScript,
    TypeScript,
    Python,
    Go,
    Shell,
    Markdown,
    Json,
    Toml,
    Yaml,
    Unknown,
}

fn detect_language(path: &Path) -> ReadLanguage {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("rs") => ReadLanguage::Rust,
        Some("js" | "jsx" | "mjs" | "cjs") => ReadLanguage::JavaScript,
        Some("ts" | "tsx") => ReadLanguage::TypeScript,
        Some("py") => ReadLanguage::Python,
        Some("go") => ReadLanguage::Go,
        Some("sh" | "bash" | "zsh") => ReadLanguage::Shell,
        Some("md" | "markdown") => ReadLanguage::Markdown,
        Some("json") => ReadLanguage::Json,
        Some("toml") => ReadLanguage::Toml,
        Some("yaml" | "yml") => ReadLanguage::Yaml,
        _ => ReadLanguage::Unknown,
    }
}

fn filter_read_content(content: &str, language: ReadLanguage, level: ReadFilterLevel) -> String {
    match level {
        ReadFilterLevel::None => content.to_string(),
        ReadFilterLevel::Minimal => strip_language_comments(content, language, false),
        ReadFilterLevel::Aggressive => strip_language_comments(content, language, true),
    }
}

fn strip_language_comments(content: &str, language: ReadLanguage, aggressive: bool) -> String {
    let mut out = Vec::new();
    let mut blank_pending = false;
    for line in content.lines() {
        let trimmed = line.trim();
        let is_comment = match language {
            ReadLanguage::Rust
            | ReadLanguage::JavaScript
            | ReadLanguage::TypeScript
            | ReadLanguage::Go => {
                trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*')
            }
            ReadLanguage::Python | ReadLanguage::Shell => trimmed.starts_with('#'),
            ReadLanguage::Markdown
            | ReadLanguage::Json
            | ReadLanguage::Toml
            | ReadLanguage::Yaml
            | ReadLanguage::Unknown => false,
        };
        if is_comment {
            continue;
        }
        if aggressive && trimmed.is_empty() {
            if !blank_pending {
                out.push(String::new());
                blank_pending = true;
            }
            continue;
        }
        blank_pending = false;
        out.push(line.to_string());
    }
    let mut rendered = out.join("\n");
    if content.ends_with('\n') && !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    rendered
}

fn apply_line_window(
    content: &str,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    language: ReadLanguage,
) -> String {
    if let Some(tail) = tail_lines {
        if tail == 0 {
            return String::new();
        }
        let lines = content.lines().collect::<Vec<_>>();
        let start = lines.len().saturating_sub(tail);
        let mut result = lines[start..].join("\n");
        if content.ends_with('\n') {
            result.push('\n');
        }
        return result;
    }

    if let Some(max) = max_lines {
        return smart_truncate_for_read(content, max, language);
    }

    content.to_string()
}

fn smart_truncate_for_read(content: &str, max_lines: usize, language: ReadLanguage) -> String {
    if max_lines == 0 {
        return String::new();
    }
    let lines = content.lines().collect::<Vec<_>>();
    if lines.len() <= max_lines {
        return content.to_string();
    }
    let mut rendered = lines
        .iter()
        .take(max_lines)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    rendered.push('\n');
    rendered.push_str(&format!(
        "... {} more lines ({:?})\n",
        lines.len() - max_lines,
        language
    ));
    rendered
}

struct SmartSourceSummary {
    line1: String,
    line2: String,
}

fn summarize_source_file(content: &str, language: ReadLanguage) -> SmartSourceSummary {
    let line_count = content.lines().count();
    let functions = extract_source_symbols(content, language, SourceSymbolKind::Function);
    let structs = extract_source_symbols(content, language, SourceSymbolKind::Type);
    let traits = extract_source_symbols(content, language, SourceSymbolKind::Trait);
    let imports = extract_source_imports(content, language);
    let patterns = detect_source_patterns(content, language);

    let language_name = language_display_name(language);
    let subject = if !structs.is_empty() && !functions.is_empty() {
        format!("{language_name} module")
    } else if !structs.is_empty() {
        format!("{language_name} data structures")
    } else if !functions.is_empty() {
        format!("{language_name} functions")
    } else {
        format!("{language_name} code")
    };

    let mut components = Vec::new();
    if !functions.is_empty() {
        components.push(format!("{} fn", functions.len()));
    }
    if !structs.is_empty() {
        components.push(format!("{} type", structs.len()));
    }
    if !traits.is_empty() {
        components.push(format!("{} trait", traits.len()));
    }
    let line1 = if components.is_empty() {
        format!("{subject} ({line_count} lines)")
    } else {
        format!("{subject} ({}) - {line_count} lines", components.join(", "))
    };

    let mut details = Vec::new();
    if !imports.is_empty() {
        details.push(format!(
            "uses: {}",
            imports.into_iter().take(3).collect::<Vec<_>>().join(", ")
        ));
    }
    if !patterns.is_empty() {
        details.push(format!("patterns: {}", patterns.join(", ")));
    }
    if details.is_empty() && !functions.is_empty() {
        details.push(format!(
            "defines: {}",
            functions.into_iter().take(3).collect::<Vec<_>>().join(", ")
        ));
    }
    SmartSourceSummary {
        line1,
        line2: if details.is_empty() {
            "General purpose code file".to_string()
        } else {
            details.join(" | ")
        },
    }
}

#[derive(Clone, Copy)]
enum SourceSymbolKind {
    Function,
    Type,
    Trait,
}

fn extract_source_symbols(
    content: &str,
    language: ReadLanguage,
    kind: SourceSymbolKind,
) -> Vec<String> {
    let pattern = match (language, kind) {
        (ReadLanguage::Rust, SourceSymbolKind::Function) => {
            r"(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)"
        }
        (ReadLanguage::Rust, SourceSymbolKind::Type) => {
            r"(?:pub\s+)?(?:struct|enum)\s+([A-Za-z_][A-Za-z0-9_]*)"
        }
        (ReadLanguage::Rust, SourceSymbolKind::Trait) => {
            r"(?:pub\s+)?trait\s+([A-Za-z_][A-Za-z0-9_]*)"
        }
        (ReadLanguage::Python, SourceSymbolKind::Function) => r"def\s+([A-Za-z_][A-Za-z0-9_]*)",
        (ReadLanguage::Python, SourceSymbolKind::Type) => r"class\s+([A-Za-z_][A-Za-z0-9_]*)",
        (ReadLanguage::TypeScript, SourceSymbolKind::Function)
        | (ReadLanguage::JavaScript, SourceSymbolKind::Function) => {
            r"(?:async\s+)?function\s+([A-Za-z_][A-Za-z0-9_]*)|(?:const|let|var)\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(?:async\s+)?\("
        }
        (ReadLanguage::TypeScript, SourceSymbolKind::Type) => {
            r"(?:interface|class|type)\s+([A-Za-z_][A-Za-z0-9_]*)"
        }
        (ReadLanguage::JavaScript, SourceSymbolKind::Type) => r"class\s+([A-Za-z_][A-Za-z0-9_]*)",
        (ReadLanguage::Go, SourceSymbolKind::Function) => {
            r"func\s+(?:\([^)]+\)\s+)?([A-Za-z_][A-Za-z0-9_]*)"
        }
        (ReadLanguage::Go, SourceSymbolKind::Type) => r"type\s+([A-Za-z_][A-Za-z0-9_]*)\s+struct",
        _ => return Vec::new(),
    };
    let re = Regex::new(pattern).expect("valid smart symbol regex");
    re.captures_iter(content)
        .filter_map(|caps| caps.get(1).or_else(|| caps.get(2)))
        .map(|item| item.as_str().to_string())
        .filter(|name| !matches!(name.as_str(), "main" | "new") && !name.starts_with("test_"))
        .take(10)
        .collect()
}

fn extract_source_imports(content: &str, language: ReadLanguage) -> Vec<String> {
    let pattern = match language {
        ReadLanguage::Rust => r"^use\s+([A-Za-z_][A-Za-z0-9_]*)",
        ReadLanguage::Python => r"^(?:from\s+(\S+)|import\s+(\S+))",
        ReadLanguage::JavaScript | ReadLanguage::TypeScript => {
            r#"(?:import.*from\s+['"]([^'"]+)['"]|require\(['"]([^'"]+)['"]\))"#
        }
        ReadLanguage::Go => r#"^\s*"([^"]+)"$"#,
        _ => return Vec::new(),
    };
    let re = Regex::new(pattern).expect("valid smart import regex");
    let mut seen = HashSet::new();
    let mut imports = Vec::new();
    for caps in re.captures_iter(content) {
        let Some(raw) = caps.get(1).or_else(|| caps.get(2)) else {
            continue;
        };
        let base = raw.as_str().split("::").next().unwrap_or(raw.as_str());
        if is_standard_import(base, language) || !seen.insert(base.to_string()) {
            continue;
        }
        imports.push(base.to_string());
    }
    imports.into_iter().take(5).collect()
}

fn is_standard_import(name: &str, language: ReadLanguage) -> bool {
    match language {
        ReadLanguage::Rust => matches!(name, "std" | "core" | "alloc"),
        ReadLanguage::Python => matches!(name, "os" | "sys" | "re" | "json" | "typing"),
        _ => false,
    }
}

fn detect_source_patterns(content: &str, language: ReadLanguage) -> Vec<String> {
    let mut patterns = Vec::new();
    if content.contains("async") && content.contains("await") {
        patterns.push("async");
    }
    match language {
        ReadLanguage::Rust => {
            if content.contains("impl") && content.contains(" for ") {
                patterns.push("trait impl");
            }
            if content.contains("#[derive") {
                patterns.push("derive");
            }
            if content.contains("Result<") || content.contains("anyhow::") {
                patterns.push("error handling");
            }
            if content.contains("#[test]") {
                patterns.push("tests");
            }
        }
        ReadLanguage::Python => {
            if content.contains("@dataclass") {
                patterns.push("dataclass");
            }
            if content.contains("def __init__") {
                patterns.push("OOP");
            }
        }
        ReadLanguage::JavaScript | ReadLanguage::TypeScript => {
            if content.contains("useState") || content.contains("useEffect") {
                patterns.push("React hooks");
            }
            if content.contains("export default") {
                patterns.push("ES modules");
            }
        }
        _ => {}
    }
    patterns.into_iter().take(3).map(str::to_string).collect()
}

fn language_display_name(language: ReadLanguage) -> &'static str {
    match language {
        ReadLanguage::Rust => "Rust",
        ReadLanguage::Python => "Python",
        ReadLanguage::JavaScript => "JavaScript",
        ReadLanguage::TypeScript => "TypeScript",
        ReadLanguage::Go => "Go",
        ReadLanguage::Shell => "Shell",
        ReadLanguage::Markdown => "Markdown",
        ReadLanguage::Json => "JSON",
        ReadLanguage::Toml => "TOML",
        ReadLanguage::Yaml => "YAML",
        ReadLanguage::Unknown => "Code",
    }
}

fn format_with_line_numbers(content: &str) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    let width = lines.len().max(1).to_string().len();
    let mut out = String::new();
    for (index, line) in lines.iter().enumerate() {
        out.push_str(&format!(
            "{:>width$} | {}\n",
            index + 1,
            line,
            width = width
        ));
    }
    out
}

fn render_env_vars(vars: &[(String, String)], filter: Option<&str>, show_all: bool) -> String {
    let sensitive_patterns = sensitive_patterns();
    let filter = filter.map(str::to_lowercase);
    let mut path_vars = Vec::new();
    let mut lang_vars = Vec::new();
    let mut cloud_vars = Vec::new();
    let mut tool_vars = Vec::new();
    let mut other_vars = Vec::new();

    for (key, value) in vars {
        if let Some(filter) = &filter {
            if !key.to_lowercase().contains(filter) {
                continue;
            }
        }
        let sensitive = sensitive_patterns
            .iter()
            .any(|pattern| key.to_lowercase().contains(pattern));
        let display_value = if sensitive && !show_all {
            mask_value(value)
        } else if value.chars().count() > 100 {
            let preview = value.chars().take(50).collect::<String>();
            format!("{preview}... ({} chars)", value.chars().count())
        } else {
            value.clone()
        };
        let entry = (key.clone(), display_value);
        if key.contains("PATH") {
            path_vars.push(entry);
        } else if is_lang_var(key) {
            lang_vars.push(entry);
        } else if is_cloud_var(key) {
            cloud_vars.push(entry);
        } else if is_tool_var(key) {
            tool_vars.push(entry);
        } else if filter.is_some() || is_interesting_var(key) {
            other_vars.push(entry);
        }
    }

    let mut out = String::new();
    append_path_vars(&mut out, &path_vars);
    append_env_section(&mut out, "Language/Runtime", &lang_vars, None);
    append_env_section(&mut out, "Cloud/Services", &cloud_vars, None);
    append_env_section(&mut out, "Tools", &tool_vars, None);
    append_env_section(&mut out, "Other", &other_vars, Some(20));
    if filter.is_none() {
        let shown = path_vars.len()
            + lang_vars.len()
            + cloud_vars.len()
            + tool_vars.len()
            + other_vars.len().min(20);
        out.push_str(&format!(
            "\nTotal: {} vars (showing {} relevant)\n",
            vars.len(),
            shown
        ));
    }
    out
}

fn append_path_vars(out: &mut String, vars: &[(String, String)]) {
    if vars.is_empty() {
        return;
    }
    out.push_str("PATH Variables:\n");
    for (key, value) in vars {
        if key == "PATH" {
            let paths = value.split(':').collect::<Vec<_>>();
            out.push_str(&format!("  PATH ({} entries):\n", paths.len()));
            for path in paths.iter().take(5) {
                out.push_str(&format!("    {path}\n"));
            }
            if paths.len() > 5 {
                out.push_str(&format!("    ... +{} more\n", paths.len() - 5));
            }
        } else {
            out.push_str(&format!("  {key}={value}\n"));
        }
    }
}

fn append_env_section(
    out: &mut String,
    label: &str,
    vars: &[(String, String)],
    limit: Option<usize>,
) {
    if vars.is_empty() {
        return;
    }
    out.push_str(&format!("\n{label}:\n"));
    let limit = limit.unwrap_or(vars.len());
    for (key, value) in vars.iter().take(limit) {
        out.push_str(&format!("  {key}={value}\n"));
    }
    if vars.len() > limit {
        out.push_str(&format!("  ... +{} more\n", vars.len() - limit));
    }
}

fn sensitive_patterns() -> HashSet<&'static str> {
    [
        "key",
        "secret",
        "password",
        "token",
        "credential",
        "auth",
        "private",
        "api_key",
        "apikey",
        "access_key",
        "jwt",
    ]
    .into_iter()
    .collect()
}

fn mask_value(value: &str) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= 4 {
        "****".to_string()
    } else {
        let prefix = chars.iter().take(2).collect::<String>();
        let suffix = chars
            .iter()
            .skip(chars.len().saturating_sub(2))
            .collect::<String>();
        format!("{prefix}****{suffix}")
    }
}

fn is_lang_var(key: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "RUST", "CARGO", "PYTHON", "PIP", "NODE", "NPM", "YARN", "DENO", "BUN", "JAVA", "MAVEN",
        "GRADLE", "GO", "GOPATH", "GOROOT", "RUBY", "GEM", "PERL", "PHP", "DOTNET", "NUGET",
    ];
    let key = key.to_uppercase();
    PATTERNS.iter().any(|pattern| key.contains(pattern))
}

fn is_cloud_var(key: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "AWS",
        "AZURE",
        "GCP",
        "GOOGLE_CLOUD",
        "DOCKER",
        "KUBERNETES",
        "K8S",
        "HELM",
        "TERRAFORM",
        "VAULT",
        "CONSUL",
        "NOMAD",
    ];
    let key = key.to_uppercase();
    PATTERNS.iter().any(|pattern| key.contains(pattern))
}

fn is_tool_var(key: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "EDITOR",
        "VISUAL",
        "SHELL",
        "TERM",
        "GIT",
        "SSH",
        "GPG",
        "BREW",
        "HOMEBREW",
        "XDG",
        "CLAUDE",
        "ANTHROPIC",
    ];
    let key = key.to_uppercase();
    PATTERNS.iter().any(|pattern| key.contains(pattern))
}

fn is_interesting_var(key: &str) -> bool {
    const PATTERNS: &[&str] = &["HOME", "USER", "LANG", "LC_", "TZ", "PWD", "OLDPWD"];
    let key = key.to_uppercase();
    PATTERNS.iter().any(|pattern| key.starts_with(pattern))
}

fn analyze_logs(content: &str) -> Result<String> {
    let timestamp_re = Regex::new(r"^\d{4}[-/]\d{2}[-/]\d{2}[T ]\d{2}:\d{2}:\d{2}[.,]?\d*\s*")?;
    let uuid_re =
        Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")?;
    let hex_re = Regex::new(r"0x[0-9a-fA-F]+")?;
    let num_re = Regex::new(r"\b\d{4,}\b")?;
    let path_re = Regex::new(r"/[\w./\-]+")?;

    let mut error_counts = std::collections::HashMap::<String, usize>::new();
    let mut warn_counts = std::collections::HashMap::<String, usize>::new();
    let mut info_counts = std::collections::HashMap::<String, usize>::new();
    let mut unique_errors = Vec::<String>::new();
    let mut unique_warnings = Vec::<String>::new();

    for line in content.lines() {
        let lower = line.to_lowercase();
        let normalized =
            normalize_log_line(line, &timestamp_re, &uuid_re, &hex_re, &num_re, &path_re);
        if lower.contains("error") || lower.contains("fatal") || lower.contains("panic") {
            let count = error_counts.entry(normalized).or_insert(0);
            if *count == 0 {
                unique_errors.push(line.to_string());
            }
            *count += 1;
        } else if lower.contains("warn") {
            let count = warn_counts.entry(normalized).or_insert(0);
            if *count == 0 {
                unique_warnings.push(line.to_string());
            }
            *count += 1;
        } else if lower.contains("info") {
            *info_counts.entry(normalized).or_insert(0) += 1;
        }
    }

    let total_errors = error_counts.values().sum::<usize>();
    let total_warnings = warn_counts.values().sum::<usize>();
    let total_info = info_counts.values().sum::<usize>();
    let mut lines = vec![
        "Log Summary".to_string(),
        format!(
            "   [error] {} errors ({} unique)",
            total_errors,
            error_counts.len()
        ),
        format!(
            "   [warn] {} warnings ({} unique)",
            total_warnings,
            warn_counts.len()
        ),
        format!("   [info] {} info messages", total_info),
        String::new(),
    ];
    append_log_group(
        &mut lines,
        "[ERRORS]",
        &error_counts,
        &unique_errors,
        (&timestamp_re, &uuid_re, &hex_re, &num_re, &path_re),
        10,
    );
    append_log_group(
        &mut lines,
        "[WARNINGS]",
        &warn_counts,
        &unique_warnings,
        (&timestamp_re, &uuid_re, &hex_re, &num_re, &path_re),
        5,
    );
    Ok(lines.join("\n"))
}

fn filter_pipe_input(filter: Option<&str>, input: &str) -> Result<String> {
    let selected = filter
        .map(|name| name.to_string())
        .unwrap_or_else(|| auto_detect_pipe_filter(input).to_string());
    match selected.as_str() {
        "grep" | "rg" => Ok(render_pipe_grep(input)),
        "find" | "fd" => Ok(render_pipe_find(input)),
        "json" => filter_json_compact(input, 4),
        "log" => analyze_logs(input),
        "cargo" | "cargo-test" => Ok(summarize_command_output(input, "cargo test", true)),
        "pytest" => Ok(summarize_command_output(input, "pytest", true)),
        "go-test" => Ok(summarize_command_output(input, "go test", true)),
        "go-build" => Ok(summarize_command_output(input, "go build", true)),
        "tsc" => Ok(summarize_command_output(input, "tsc", true)),
        "vitest" => Ok(summarize_command_output(input, "vitest", true)),
        "mypy" => Ok(summarize_command_output(input, "mypy", true)),
        "ruff-check" => Ok(summarize_command_output(input, "ruff check", true)),
        "ruff-format" => Ok(summarize_command_output(input, "ruff format", true)),
        "prettier" => Ok(summarize_command_output(input, "prettier", true)),
        "auto" => Ok(input.to_string()),
        other => bail!(
            "unknown pipe filter '{other}' (available: cargo-test, pytest, go-test, go-build, tsc, vitest, grep, rg, find, fd, json, log, mypy, ruff-check, ruff-format, prettier)"
        ),
    }
}

fn auto_detect_pipe_filter(input: &str) -> &'static str {
    let first = input.lines().take(8).collect::<Vec<_>>().join("\n");
    let lower = first.to_lowercase();
    let trimmed = first.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        "json"
    } else if lower.contains("test result:") || lower.contains("=== test session starts") {
        "cargo-test"
    } else if first.lines().any(is_grep_like_line) {
        "grep"
    } else if looks_like_find_output(&first) {
        "find"
    } else if lower.contains("error:") || lower.contains("warn:") || lower.contains("panic") {
        "log"
    } else {
        "auto"
    }
}

fn render_pipe_grep(input: &str) -> String {
    let mut entries = Vec::<(&str, usize, &str)>::new();
    for line in input.lines() {
        let Some((path, rest)) = line.split_once(':') else {
            continue;
        };
        let Some((line_no, text)) = rest.split_once(':') else {
            continue;
        };
        if let Ok(line_no) = line_no.parse::<usize>() {
            entries.push((path, line_no, text));
        }
    }
    if entries.is_empty() {
        return input.to_string();
    }
    let mut files = HashSet::new();
    for (path, _, _) in &entries {
        files.insert(*path);
    }
    let mut out = format!("{} matches in {} files:\n\n", entries.len(), files.len());
    for (path, line_no, text) in entries.iter().take(50) {
        out.push_str(&format!(
            "{path}:{line_no}:{}\n",
            compact_for_log(text.trim(), 160)
        ));
    }
    if entries.len() > 50 {
        out.push_str(&format!("[+{} more]\n", entries.len() - 50));
    }
    out
}

fn render_pipe_find(input: &str) -> String {
    let paths = input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return input.to_string();
    }
    let mut out = format!("{} paths:\n", paths.len());
    for path in paths.iter().take(80) {
        out.push_str(path);
        out.push('\n');
    }
    if paths.len() > 80 {
        out.push_str(&format!("[+{} more]\n", paths.len() - 80));
    }
    out
}

fn is_grep_like_line(line: &str) -> bool {
    let Some((_path, rest)) = line.split_once(':') else {
        return false;
    };
    let Some((line_no, _text)) = rest.split_once(':') else {
        return false;
    };
    line_no.parse::<usize>().is_ok()
}

fn looks_like_find_output(input: &str) -> bool {
    let mut count = 0usize;
    for line in input.lines().filter(|line| !line.trim().is_empty()) {
        count += 1;
        let trimmed = line.trim();
        if trimmed.contains(':')
            || !(trimmed.starts_with('.') || trimmed.starts_with('/') || trimmed.contains('/'))
        {
            return false;
        }
    }
    count >= 3
}

fn append_log_group(
    lines: &mut Vec<String>,
    label: &str,
    counts: &std::collections::HashMap<String, usize>,
    originals: &[String],
    regexes: (&Regex, &Regex, &Regex, &Regex, &Regex),
    limit: usize,
) {
    if originals.is_empty() {
        return;
    }
    lines.push(label.to_string());
    let mut sorted = counts.iter().collect::<Vec<_>>();
    sorted.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    for (normalized, count) in sorted.iter().take(limit) {
        let original = originals
            .iter()
            .find(|line| {
                normalize_log_line(line, regexes.0, regexes.1, regexes.2, regexes.3, regexes.4)
                    == **normalized
            })
            .map(String::as_str)
            .unwrap_or(normalized);
        let truncated = compact_for_log(original, 100);
        if **count > 1 {
            lines.push(format!("   [x{}] {truncated}", count));
        } else {
            lines.push(format!("   {truncated}"));
        }
    }
    if sorted.len() > limit {
        lines.push(format!(
            "   ... +{} more unique entries",
            sorted.len() - limit
        ));
    }
    lines.push(String::new());
}

fn normalize_log_line(
    line: &str,
    timestamp_re: &Regex,
    uuid_re: &Regex,
    hex_re: &Regex,
    num_re: &Regex,
    path_re: &Regex,
) -> String {
    let mut normalized = timestamp_re.replace_all(line, "").to_string();
    normalized = uuid_re.replace_all(&normalized, "<UUID>").to_string();
    normalized = hex_re.replace_all(&normalized, "<HEX>").to_string();
    normalized = num_re.replace_all(&normalized, "<NUM>").to_string();
    normalized = path_re.replace_all(&normalized, "<PATH>").to_string();
    normalized.trim().to_string()
}

fn compact_for_log(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_string()
    } else {
        let budget = max_chars.saturating_sub(3);
        format!("{}...", value.chars().take(budget).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_compact_truncates_values_and_summarizes_arrays() {
        let rendered = filter_json_compact(
            &serde_json::json!({
                "long": "x".repeat(100),
                "items": [1, 2, 3, 4, 5, 6],
            })
            .to_string(),
            5,
        )
        .unwrap();
        assert!(rendered.contains("long: \""));
        assert!(rendered.contains("...\""));
        assert!(rendered.contains("[1, ... +5 more]"));
    }

    #[test]
    fn read_filters_comments_and_applies_line_windows() {
        let content = "// comment\nfn main() {}\n\n// another\nfn next() {}\n";
        let filtered = filter_read_content(content, ReadLanguage::Rust, ReadFilterLevel::Minimal);
        assert!(!filtered.contains("// comment"));
        assert!(filtered.contains("fn main()"));
        let windowed = apply_line_window(&filtered, Some(1), None, ReadLanguage::Rust);
        assert!(windowed.starts_with("fn main()"));
        assert!(windowed.contains("more lines"));
    }

    #[test]
    fn read_line_numbers_are_stable_width() {
        let rendered = format_with_line_numbers("alpha\nbeta\n");
        assert!(rendered.contains("1 | alpha"));
        assert!(rendered.contains("2 | beta"));
    }

    #[test]
    fn smart_summarizes_source_file_with_local_heuristics() {
        let summary = summarize_source_file(
            r#"
use anyhow::Result;

#[derive(Debug)]
pub struct Config {
    name: String,
}

pub fn load_config() -> Result<Config> {
    Ok(Config { name: "demo".to_string() })
}
"#,
            ReadLanguage::Rust,
        );
        assert!(summary.line1.contains("Rust module"));
        assert!(summary.line1.contains("1 fn"));
        assert!(summary.line1.contains("1 type"));
        assert!(summary.line2.contains("derive"));
        assert!(summary.line2.contains("error handling"));
    }

    #[test]
    fn summary_detects_json_and_test_output() {
        let json_summary = summarize_command_output(r#"{"ok":true,"items":[1,2]}"#, "printf", true);
        assert!(json_summary.contains("JSON Summary:"));
        assert!(json_summary.contains("items:"));
        let test_summary = summarize_command_output(
            "test result: FAILED. 1 passed; 2 failed; 3 ignored",
            "cargo test",
            false,
        );
        assert!(test_summary.contains("[FAIL] Command: cargo test"));
        assert!(test_summary.contains("[ok] 1 passed"));
        assert!(test_summary.contains("[FAIL] 2 failed"));
        assert!(test_summary.contains("skip 3 skipped"));
    }

    #[test]
    fn find_parses_native_and_compact_args() {
        let native = parse_find_args(&[
            ".".to_string(),
            "-name".to_string(),
            "*.rs".to_string(),
            "-type".to_string(),
            "f".to_string(),
            "-maxdepth".to_string(),
            "2".to_string(),
        ])
        .unwrap();
        assert_eq!(native.pattern, "*.rs");
        assert_eq!(native.file_type, "f");
        assert_eq!(native.max_depth, Some(2));

        let compact = parse_find_args(&[
            "*.md".to_string(),
            "docs".to_string(),
            "--max".to_string(),
            "5".to_string(),
        ])
        .unwrap();
        assert_eq!(compact.pattern, "*.md");
        assert_eq!(compact.path, PathBuf::from("docs"));
        assert_eq!(compact.max_results, 5);
    }

    #[test]
    fn find_renders_limited_matches() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src").join("a.rs"), "").unwrap();
        fs::write(dir.path().join("src").join("b.rs"), "").unwrap();
        fs::write(dir.path().join("src").join("c.txt"), "").unwrap();
        let rendered = render_find_results(&ParsedFindArgs {
            pattern: "*.rs".to_string(),
            path: dir.path().to_path_buf(),
            max_results: 1,
            max_depth: Some(3),
            file_type: "f".to_string(),
            case_insensitive: false,
        })
        .unwrap();
        assert!(rendered.contains("2 match(es)"));
        assert!(rendered.contains("[+1 more]"));
    }

    #[test]
    fn grep_groups_matches_and_honors_file_type() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src").join("a.rs"),
            "fn alpha() {}\nfn beta() {}\n",
        )
        .unwrap();
        fs::write(dir.path().join("src").join("b.txt"), "fn ignored\n").unwrap();
        let rendered = render_grep_results(&GrepArgs {
            pattern: "fn".to_string(),
            path: dir.path().to_path_buf(),
            max_len: 80,
            max: 1,
            context_only: false,
            file_type: Some("rs".to_string()),
            engine: GrepEngine::Legacy,
            json: false,
            pretty: false,
        })
        .unwrap();
        assert!(rendered.contains("2 matches in 1 files"));
        assert!(rendered.contains("a.rs:1:fn alpha() {}"));
        assert!(rendered.contains("[+1 more]"));
        assert!(!rendered.contains("ignored"));
    }

    #[test]
    fn json_schema_renders_types_without_values() {
        let rendered = filter_json_schema(
            r#"{"count":42,"items":[{"url":"https://example.com","enabled":true}]}"#,
            5,
        )
        .unwrap();
        assert!(rendered.contains("count: int"));
        assert!(rendered.contains("enabled: bool"));
        assert!(rendered.contains("url: url"));
        assert!(!rendered.contains("example.com"));
    }

    #[test]
    fn json_extension_rejects_cargo_toml_with_deps_hint() {
        let err = validate_json_extension(Path::new("Cargo.toml")).unwrap_err();
        assert!(err.to_string().contains("not a JSON file"));
        assert!(err.to_string().contains("Packet28 deps"));
    }

    #[test]
    fn env_masks_sensitive_values_and_limits_path() {
        let vars = vec![
            (
                "AWS_SECRET_ACCESS_KEY".to_string(),
                "supersecrettoken".to_string(),
            ),
            ("PATH".to_string(), "/a:/b:/c:/d:/e:/f".to_string()),
            ("RUST_LOG".to_string(), "debug".to_string()),
        ];
        let rendered = render_env_vars(&vars, None, false);
        assert!(rendered.contains("AWS_SECRET_ACCESS_KEY=su****en"));
        assert!(rendered.contains("PATH (6 entries)"));
        assert!(rendered.contains("... +1 more"));
        assert!(rendered.contains("RUST_LOG=debug"));
    }

    #[test]
    fn env_filter_includes_other_matching_vars() {
        let vars = vec![("CUSTOM_TOKEN".to_string(), "abcde".to_string())];
        let rendered = render_env_vars(&vars, Some("custom"), false);
        assert!(rendered.contains("CUSTOM_TOKEN=ab****de"));
        assert!(!rendered.contains("Total:"));
    }

    #[test]
    fn deps_summarizes_multiple_manifest_types() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[dependencies]\nanyhow = \"1\"\nserde = { version = \"1\" }\n[dev-dependencies]\ntempfile = \"3\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"name":"demo","version":"0.1.0","dependencies":{"react":"18"},"devDependencies":{"vite":"5"}}"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("requirements.txt"),
            "pytest==8\n# ignored\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("go.mod"),
            "module example.com/demo\ngo 1.22\nrequire github.com/acme/lib v1.2.3\n",
        )
        .unwrap();
        let rendered = summarize_deps_dir(dir.path()).unwrap();
        assert!(rendered.contains("Rust (Cargo.toml):"));
        assert!(rendered.contains("anyhow (1)"));
        assert!(rendered.contains("Node.js (package.json):"));
        assert!(rendered.contains("demo @ 0.1.0"));
        assert!(rendered.contains("pytest==8"));
        assert!(rendered.contains("example.com/demo (go 1.22)"));
    }

    #[test]
    fn log_analysis_normalizes_variable_ids_and_paths() {
        let rendered = analyze_logs(
            "2026-05-12T01:00:00 ERROR failed request 1001 at /tmp/a\n\
             2026-05-12T01:00:01 ERROR failed request 1002 at /tmp/b\n\
             2026-05-12T01:00:02 WARN retry 2001\n\
             2026-05-12T01:00:03 INFO healthy\n",
        )
        .unwrap();
        assert!(rendered.contains("[error] 2 errors (1 unique)"));
        assert!(rendered.contains("[warn] 1 warnings (1 unique)"));
        assert!(rendered.contains("[info] 1 info messages"));
        assert!(rendered.contains("[x2]"));
    }
}
