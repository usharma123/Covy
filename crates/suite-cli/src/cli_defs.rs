use clap::{Args, Parser, Subcommand};

use crate::{
    cmd_agent_prompt, cmd_build, cmd_compact, cmd_context, cmd_cover, cmd_daemon, cmd_dashboard,
    cmd_diff, cmd_discover, cmd_doctor, cmd_feedback, cmd_graph, cmd_guard, cmd_hook, cmd_impact,
    cmd_init, cmd_learn, cmd_map, cmd_map_query, cmd_map_repo, cmd_mcp, cmd_memory, cmd_packet,
    cmd_proxy, cmd_run, cmd_setup, cmd_shard, cmd_shell, cmd_stack, cmd_system, cmd_transcript,
    cmd_verify, cmd_wakeup,
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
    Npm(cmd_system::ToolArgs),
    /// Run npx with compact output
    Npx(cmd_system::ToolArgs),
    /// Run the TypeScript compiler with compact output
    Tsc(cmd_system::ToolArgs),
    /// Run Vitest with compact output
    Vitest(cmd_system::ToolArgs),
    /// Run pytest with compact output
    Pytest(cmd_system::ToolArgs),
    /// Run Ruff with compact output
    Ruff(cmd_system::ToolArgs),
    /// Run ls with compact output
    Ls(cmd_system::ToolArgs),
    /// Run wc with compact output
    Wc(cmd_system::ToolArgs),
    /// Run wget with compact output
    Wget(cmd_system::ToolArgs),
    /// Run curl with compact output
    Curl(cmd_system::ToolArgs),
    /// Run docker with compact output
    Docker(cmd_system::ToolArgs),
    /// Run kubectl with compact output
    Kubectl(cmd_system::ToolArgs),
    /// Run Cargo with compact output
    Cargo(cmd_system::ToolArgs),
    /// Run Go with compact output
    Go(cmd_system::ToolArgs),
    /// Run dotnet with compact output
    Dotnet(cmd_system::ToolArgs),
    /// Run golangci-lint with compact output
    #[command(name = "golangci-lint")]
    GolangciLint(cmd_system::ToolArgs),
    /// Run Gradle wrapper with compact output
    #[command(name = "gradlew")]
    Gradlew(cmd_system::ToolArgs),
    /// Run Rake with compact output
    Rake(cmd_system::ToolArgs),
    /// Run RSpec with compact output
    Rspec(cmd_system::ToolArgs),
    /// Run RuboCop with compact output
    Rubocop(cmd_system::ToolArgs),
    /// Run mypy with compact output
    Mypy(cmd_system::ToolArgs),
    /// Run pip with compact output
    Pip(cmd_system::ToolArgs),
    /// Run pnpm with compact output
    Pnpm(cmd_system::ToolArgs),
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
    /// Plan how Packet28 would reduce or pass through a command
    Rewrite(cmd_compact::RewriteArgs),
    /// Inspect Packet28 task sessions
    Session(cmd_compact::SessionArgs),
    /// Packet artifact utilities
    Packet(cmd_packet::PacketArgs),
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
    /// Show local Packet28 savings, memory, feedback, graph, and integration health
    Dashboard(cmd_dashboard::DashboardArgs),
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
