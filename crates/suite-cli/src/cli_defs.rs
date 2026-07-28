use clap::{Args, Parser, Subcommand};

use crate::{
    cmd_agent_prompt, cmd_build, cmd_compact, cmd_context, cmd_cover, cmd_daemon, cmd_dashboard,
    cmd_diff, cmd_digest, cmd_discover, cmd_doctor, cmd_feedback, cmd_graph, cmd_guard, cmd_hook,
    cmd_hypothesis, cmd_impact, cmd_init, cmd_learn, cmd_map, cmd_map_query, cmd_map_repo, cmd_mcp,
    cmd_memory, cmd_packet, cmd_plan, cmd_proxy, cmd_run, cmd_setup, cmd_shard, cmd_shell,
    cmd_stack, cmd_system, cmd_transcript, cmd_verify, cmd_wakeup,
};

#[derive(Parser)]
#[command(
    name = "Packet28",
    version = env!("PACKET28_VERSION"),
    about = "Umbrella platform CLI for suite domains",
    after_help = "Examples:\n  Packet28\n  Packet28 setup --runtime all --yes\n  Packet28 diff analyze --coverage tests/fixtures/lcov/basic.info --base HEAD --head HEAD --json\n  Packet28 gain --task-id task-123 --json\n  Packet28 agent-prompt --format claude\n  Packet28 daemon status --root . --json\n  Packet28 doctor --root . --json\n  Packet28 context store stats --json\n  Packet28 context recall --query \"missing mappings in parser\" --json"
)]
pub struct Cli {
    /// Path to config file
    #[arg(long, global = true, default_value = "covy.toml")]
    pub config: String,

    /// Write stdout output to a file instead of the terminal
    #[arg(long)]
    pub output: Option<String>,

    /// Route supported command execution through packet28d
    #[arg(long, global = true)]
    pub via_daemon: bool,

    /// Workspace root that owns the packet28d socket/runtime for routed commands
    #[arg(long, global = true)]
    pub daemon_root: Option<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Coverage domain commands
    Cover(CoverArgs),
    /// Diff domain commands
    Diff(DiffArgs),
    /// Test domain commands
    Test(TestArgs),
    /// Guard/policy domain commands
    Guard(GuardArgs),
    /// Context assembly domain commands
    Context(ContextArgs),
    /// Stack trace / failure log reduction commands
    Stack(StackArgs),
    /// Build diagnostics reduction commands
    Build(BuildArgs),
    /// Repo mapping commands
    Map(MapArgs),
    /// Safe command proxy/reduction commands
    Proxy(cmd_proxy::ProxyArgs),
    /// Aggregate estimated token savings across recorded task invocations
    Gain(cmd_compact::AnalyticsArgs),
    /// Merge ccusage spend with Packet28 recorded savings
    CcEconomics(cmd_compact::CcEconomicsArgs),
    /// RTK-style compact command surface and analytics
    Compact(cmd_compact::CompactArgs),
    /// Read files with optional line windows, line numbers, and lightweight filtering
    Read(cmd_system::ReadArgs),
    /// Run a command and emit a compact heuristic summary of its output
    Summary(cmd_system::SummaryArgs),
    /// Run a command and emphasize error-oriented compact output
    Err(cmd_system::SummaryArgs),
    /// Run Prettier and summarize formatting output
    Prettier(cmd_system::FormatterArgs),
    /// Run a formatter such as prettier, black, ruff, or biome with compact output
    Format(cmd_system::FormatterArgs),
    /// Run npm with compact output
    #[command(hide = true)]
    Npm(cmd_system::ToolArgs),
    /// Run npx with compact output
    #[command(hide = true)]
    Npx(cmd_system::ToolArgs),
    /// Run the TypeScript compiler with compact output
    #[command(hide = true)]
    Tsc(cmd_system::ToolArgs),
    /// Run Vitest with compact output
    #[command(hide = true)]
    Vitest(cmd_system::ToolArgs),
    /// Run pytest with compact output
    #[command(hide = true)]
    Pytest(cmd_system::ToolArgs),
    /// Run Ruff with compact output
    #[command(hide = true)]
    Ruff(cmd_system::ToolArgs),
    /// Run ls with compact output
    #[command(hide = true)]
    Ls(cmd_system::ToolArgs),
    /// Run tree with compact output
    #[command(hide = true)]
    Tree(cmd_system::ToolArgs),
    /// Run wc with compact output
    #[command(hide = true)]
    Wc(cmd_system::ToolArgs),
    /// Run wget with compact output
    #[command(hide = true)]
    Wget(cmd_system::ToolArgs),
    /// Run curl with compact output
    #[command(hide = true)]
    Curl(cmd_system::ToolArgs),
    /// Run docker with compact output
    #[command(hide = true)]
    Docker(cmd_system::ToolArgs),
    /// Run kubectl with compact output
    #[command(hide = true)]
    Kubectl(cmd_system::ToolArgs),
    /// Run gh with compact output
    #[command(hide = true)]
    Gh(cmd_system::ToolArgs),
    /// Run glab with compact output
    #[command(hide = true)]
    Glab(cmd_system::ToolArgs),
    /// Run aws with compact output
    #[command(hide = true)]
    Aws(cmd_system::ToolArgs),
    /// Run psql with compact output
    #[command(hide = true)]
    Psql(cmd_system::ToolArgs),
    /// Run git with compact output
    #[command(hide = true)]
    Git(cmd_system::ToolArgs),
    /// Run Graphite gt with compact output
    #[command(hide = true)]
    Gt(cmd_system::ToolArgs),
    /// Run Cargo with compact output
    #[command(hide = true)]
    Cargo(cmd_system::ToolArgs),
    /// Run Go with compact output
    #[command(hide = true)]
    Go(cmd_system::ToolArgs),
    /// Run dotnet with compact output
    #[command(hide = true)]
    Dotnet(cmd_system::ToolArgs),
    /// Run golangci-lint with compact output
    #[command(name = "golangci-lint")]
    #[command(hide = true)]
    GolangciLint(cmd_system::ToolArgs),
    /// Run Gradle wrapper with compact output
    #[command(name = "gradlew")]
    #[command(hide = true)]
    Gradlew(cmd_system::ToolArgs),
    /// Run Rake with compact output
    #[command(hide = true)]
    Rake(cmd_system::ToolArgs),
    /// Run RSpec with compact output
    #[command(hide = true)]
    Rspec(cmd_system::ToolArgs),
    /// Run RuboCop with compact output
    #[command(hide = true)]
    Rubocop(cmd_system::ToolArgs),
    /// Run mypy with compact output
    #[command(hide = true)]
    Mypy(cmd_system::ToolArgs),
    /// Run pip with compact output
    #[command(hide = true)]
    Pip(cmd_system::ToolArgs),
    /// Run pnpm with compact output
    #[command(hide = true)]
    Pnpm(cmd_system::ToolArgs),
    /// Run Bundler with compact output
    #[command(hide = true)]
    Bundle(cmd_system::ToolArgs),
    /// Run Homebrew with compact output
    #[command(hide = true)]
    Brew(cmd_system::ToolArgs),
    /// Run Composer with compact output
    #[command(hide = true)]
    Composer(cmd_system::ToolArgs),
    /// Run df with compact output
    #[command(hide = true)]
    Df(cmd_system::ToolArgs),
    /// Run du with compact output
    #[command(hide = true)]
    Du(cmd_system::ToolArgs),
    /// Run make with compact output
    #[command(hide = true)]
    Make(cmd_system::ToolArgs),
    /// Run Maven with compact output
    #[command(hide = true)]
    Mvn(cmd_system::ToolArgs),
    /// Run Poetry with compact output
    #[command(hide = true)]
    Poetry(cmd_system::ToolArgs),
    /// Run uv with compact output
    #[command(hide = true)]
    Uv(cmd_system::ToolArgs),
    /// Run Terraform with compact output
    #[command(hide = true)]
    Terraform(cmd_system::ToolArgs),
    /// Run OpenTofu with compact output
    #[command(hide = true)]
    Tofu(cmd_system::ToolArgs),
    /// Run ansible-playbook with compact output
    #[command(name = "ansible-playbook")]
    #[command(hide = true)]
    AnsiblePlaybook(cmd_system::ToolArgs),
    /// Run fail2ban-client with compact output
    #[command(name = "fail2ban-client")]
    #[command(hide = true)]
    Fail2banClient(cmd_system::ToolArgs),
    /// Run gcloud with compact output
    #[command(hide = true)]
    Gcloud(cmd_system::ToolArgs),
    /// Run hadolint with compact output
    #[command(hide = true)]
    Hadolint(cmd_system::ToolArgs),
    /// Run Helm with compact output
    #[command(hide = true)]
    Helm(cmd_system::ToolArgs),
    /// Run iptables with compact output
    #[command(hide = true)]
    Iptables(cmd_system::ToolArgs),
    /// Run markdownlint with compact output
    #[command(hide = true)]
    Markdownlint(cmd_system::ToolArgs),
    /// Run Mix with compact output
    #[command(hide = true)]
    Mix(cmd_system::ToolArgs),
    /// Run ping with compact output
    #[command(hide = true)]
    Ping(cmd_system::ToolArgs),
    /// Run PlatformIO pio with compact output
    #[command(hide = true)]
    Pio(cmd_system::ToolArgs),
    /// Run pre-commit with compact output
    #[command(name = "pre-commit")]
    #[command(hide = true)]
    PreCommit(cmd_system::ToolArgs),
    /// Run ps with compact output
    #[command(hide = true)]
    Ps(cmd_system::ToolArgs),
    /// Run Quarto with compact output
    #[command(hide = true)]
    Quarto(cmd_system::ToolArgs),
    /// Run rsync with compact output
    #[command(hide = true)]
    Rsync(cmd_system::ToolArgs),
    /// Run ShellCheck with compact output
    #[command(hide = true)]
    Shellcheck(cmd_system::ToolArgs),
    /// Run Shopify CLI with compact output
    #[command(hide = true)]
    Shopify(cmd_system::ToolArgs),
    /// Run sops with compact output
    #[command(hide = true)]
    Sops(cmd_system::ToolArgs),
    /// Run systemctl with compact output
    #[command(hide = true)]
    Systemctl(cmd_system::ToolArgs),
    /// Run Trunk with compact output
    #[command(hide = true)]
    Trunk(cmd_system::ToolArgs),
    /// Run yamllint with compact output
    #[command(hide = true)]
    Yamllint(cmd_system::ToolArgs),
    /// Run Liquibase with compact output
    #[command(hide = true)]
    Liquibase(cmd_system::ToolArgs),
    /// Run basedpyright with compact output
    #[command(hide = true)]
    Basedpyright(cmd_system::ToolArgs),
    /// Run Biome with compact output
    #[command(hide = true)]
    Biome(cmd_system::ToolArgs),
    /// Run GCC with compact output
    #[command(hide = true)]
    Gcc(cmd_system::ToolArgs),
    /// Run G++ with compact output
    #[command(name = "g++")]
    #[command(hide = true)]
    Gpp(cmd_system::ToolArgs),
    /// Run Gradle with compact output
    #[command(hide = true)]
    Gradle(cmd_system::ToolArgs),
    /// Run Jira CLI with compact output
    #[command(hide = true)]
    Jira(cmd_system::ToolArgs),
    /// Run Jujutsu jj with compact output
    #[command(hide = true)]
    Jj(cmd_system::ToolArgs),
    /// Run jq with compact output
    #[command(hide = true)]
    Jq(cmd_system::ToolArgs),
    /// Run just with compact output
    #[command(hide = true)]
    Just(cmd_system::ToolArgs),
    /// Run mise with compact output
    #[command(hide = true)]
    Mise(cmd_system::ToolArgs),
    /// Run Nx with compact output
    #[command(hide = true)]
    Nx(cmd_system::ToolArgs),
    /// Run Ollama with compact output
    #[command(hide = true)]
    Ollama(cmd_system::ToolArgs),
    /// Run oxlint with compact output
    #[command(hide = true)]
    Oxlint(cmd_system::ToolArgs),
    /// Run skopeo with compact output
    #[command(hide = true)]
    Skopeo(cmd_system::ToolArgs),
    /// Run ssh with compact output
    #[command(hide = true)]
    Ssh(cmd_system::ToolArgs),
    /// Run stat with compact output
    #[command(hide = true)]
    Stat(cmd_system::ToolArgs),
    /// Run Swift with compact output
    #[command(hide = true)]
    Swift(cmd_system::ToolArgs),
    /// Run go-task task with compact output
    #[command(hide = true)]
    Task(cmd_system::ToolArgs),
    /// Run Turborepo turbo with compact output
    #[command(hide = true)]
    Turbo(cmd_system::ToolArgs),
    /// Run ty with compact output
    #[command(hide = true)]
    Ty(cmd_system::ToolArgs),
    /// Run xcodebuild with compact output
    #[command(hide = true)]
    Xcodebuild(cmd_system::ToolArgs),
    /// Run yadm with compact output
    #[command(hide = true)]
    Yadm(cmd_system::ToolArgs),
    /// Generate a local two-line heuristic summary of a source file
    Smart(cmd_system::SmartArgs),
    /// Find files or directories with compact output
    Find(cmd_system::FindArgs),
    /// Search files with grouped compact output
    Grep(cmd_system::GrepArgs),
    /// Compactly inspect JSON files or stdin while preserving raw values by default
    Json(cmd_system::JsonArgs),
    /// Summarize project dependency manifests
    Deps(cmd_system::DepsArgs),
    /// Show categorized environment variables with secrets masked by default
    Env(cmd_system::EnvArgs),
    /// Deduplicate log output and summarize errors, warnings, and info lines
    Log(cmd_system::LogArgs),
    /// Read stdin and apply a compact output filter
    Pipe(cmd_system::PipeArgs),
    /// Plan how Packet28 would reduce or pass through a command
    Rewrite(cmd_compact::RewriteArgs),
    /// Inspect Packet28 task sessions
    Session(cmd_compact::SessionArgs),
    /// Packet artifact utilities
    Packet(cmd_packet::PacketArgs),
    /// Broker plan validation commands
    Plan(cmd_plan::PlanArgs),
    /// Local SQLite memory commands
    Memory(cmd_memory::MemoryArgs),
    /// Local feedback correction commands
    Feedback(cmd_feedback::FeedbackArgs),
    /// Local concept graph commands
    Graph(cmd_graph::GraphArgs),
    /// Local transcript session and message commands
    Transcript(cmd_transcript::TranscriptArgs),
    /// Summarize local memory, feedback, graph, and store stats for a new agent turn
    Wakeup(cmd_wakeup::WakeupArgs),
    /// Record, confirm, reject, and list active task hypotheses
    Hypothesis(cmd_hypothesis::HypothesisArgs),
    /// Show local Packet28 savings, memory, feedback, graph, and integration health
    Dashboard(cmd_dashboard::DashboardArgs),
    /// Rank local context anomalies that could mislead an agent
    Digest(cmd_digest::DigestArgs),
    /// Emit repo-local agent instruction fragments that describe how to use Packet28
    AgentPrompt(cmd_agent_prompt::AgentPromptArgs),
    /// Run Packet28 as an MCP stdio server
    Mcp(cmd_mcp::McpArgs),
    /// Run Packet28-managed runtime hook handlers
    Hook(cmd_hook::HookArgs),
    /// Daemon lifecycle and task commands
    Daemon(cmd_daemon::DaemonArgs),
    /// Verify Packet28 daemon, index, MCP, notifications, and broker round-trip health
    Doctor(cmd_doctor::DoctorArgs),
    /// Configure Packet28 for your agent runtimes (Claude Code, Cursor, Codex, Windsurf)
    Setup(cmd_setup::SetupArgs),
    /// Initialize Packet28 for a specific agent runtime
    Init(cmd_init::InitArgs),
    /// Launch an agent command through the Packet28 runtime backend
    Run(cmd_run::RunArgs),
    /// Verify Packet28 compatibility artifacts and inline tests
    Verify(cmd_verify::VerifyArgs),
    /// Launch a shell or command with the instruction virtualizer preloaded
    Shell(cmd_shell::ShellArgs),
    /// Scan sessions for command patterns and savings opportunities
    Discover(cmd_discover::DiscoverArgs),
    /// Learn error→correction patterns from session history
    Learn(cmd_learn::LearnArgs),
}

#[derive(Args)]
pub struct CoverArgs {
    #[command(subcommand)]
    pub command: CoverCommands,
}

#[derive(Subcommand)]
pub enum CoverCommands {
    /// Analyze coverage quality gate
    Check(cmd_cover::CheckArgs),
}

#[derive(Args)]
pub struct DiffArgs {
    #[command(subcommand)]
    pub command: DiffCommands,
}

#[derive(Subcommand)]
pub enum DiffCommands {
    /// Analyze a git diff and evaluate quality gate
    Analyze(cmd_diff::AnalyzeArgs),
}

#[derive(Args)]
pub struct TestArgs {
    #[command(subcommand)]
    pub command: TestCommands,
}

#[derive(Subcommand)]
pub enum TestCommands {
    /// Compute impacted tests from a git diff
    Impact(cmd_impact::ImpactArgs),
    /// Plan test shard allocations
    Shard(cmd_shard::ShardArgs),
    /// Build test impact map artifacts
    Map(cmd_map::MapArgs),
}

#[derive(Args)]
pub struct GuardArgs {
    #[command(subcommand)]
    pub command: GuardCommands,
}

#[derive(Subcommand)]
pub enum GuardCommands {
    /// Validate guard policy config (context.yaml) shape and rule syntax
    Validate(cmd_guard::ValidateArgs),
    /// Evaluate one packet against guard policy config
    Check(cmd_guard::CheckArgs),
}

#[derive(Args)]
#[command(
    after_help = "Examples:\n  Packet28 context assemble --packet a.json --packet b.json --context-config context.yaml\n  Packet28 context store list --root . --limit 20\n  Packet28 context recall --query \"what changed in parser\" --limit 5\n  Packet28 context manage --task-id task-123 --budget-tokens 4000 --budget-bytes 32000"
)]
pub struct ContextArgs {
    #[command(subcommand)]
    pub command: ContextCommands,
}

#[derive(Subcommand)]
pub enum ContextCommands {
    /// Merge multiple reducer packets into a bounded final packet
    #[command(alias = "merge")]
    Assemble(cmd_context::AssembleArgs),
    /// Correlate multiple packets into a synthesized insight packet
    Correlate(cmd_context::CorrelateArgs),
    /// Produce budget-aware task context management guidance
    Manage(cmd_context::ManageArgs),
    /// Write and inspect agent task state
    State(cmd_context::StateArgs),
    /// Query and manage persisted context store entries
    Store(cmd_context::StoreArgs),
    /// Recall prior context entries by semantic/lexical query
    Recall(cmd_context::RecallArgs),
}

#[derive(Args)]
pub struct StackArgs {
    #[command(subcommand)]
    pub command: StackCommands,
}

#[derive(Subcommand)]
pub enum StackCommands {
    /// Parse stack traces/failing logs into deduped failure packets
    Slice(cmd_stack::SliceArgs),
}

#[derive(Args)]
pub struct BuildArgs {
    #[command(subcommand)]
    pub command: BuildCommands,
}

#[derive(Subcommand)]
pub enum BuildCommands {
    /// Parse compiler/linter output into deduped build diagnostic packets
    Reduce(cmd_build::ReduceArgs),
}

#[derive(Args)]
pub struct MapArgs {
    #[command(subcommand)]
    pub command: MapCommands,
}

#[derive(Subcommand)]
pub enum MapCommands {
    /// Build deterministic repo map packet
    Repo(cmd_map_repo::RepoArgs),
    /// Query cached repo symbols with low-token output
    Query(cmd_map_query::QueryArgs),
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn top_level_help_hides_reducer_tool_wrappers() {
        let mut help = Vec::new();
        Cli::command().write_long_help(&mut help).unwrap();
        let help = String::from_utf8(help).unwrap();

        assert!(!help.contains("Run ping with compact output"));
        assert!(!help.contains("Run npm with compact output"));
        assert!(help.contains("Run Packet28 as an MCP stdio server"));
        assert!(help.contains("Run Packet28-managed runtime hook handlers"));
    }

    #[test]
    fn hidden_reducer_tool_wrappers_remain_parseable() {
        let cli = Cli::try_parse_from(["Packet28", "ping", "127.0.0.1"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Ping(_))));
    }
}
