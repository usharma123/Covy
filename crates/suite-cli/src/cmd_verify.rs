use anyhow::Result;
use clap::{Args, Subcommand};
use serde_json::json;

#[derive(Args)]
pub struct VerifyArgs {
    #[command(subcommand)]
    pub command: VerifyCommands,
}

#[derive(Subcommand)]
pub enum VerifyCommands {
    /// Run inline tests from Packet28/RTK-compatible TOML output filters
    Filters(FilterVerifyArgs),
}

#[derive(Args)]
pub struct FilterVerifyArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub filter: Option<String>,
    #[arg(long)]
    pub require_all: bool,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

pub fn run(args: VerifyArgs) -> Result<i32> {
    match args.command {
        VerifyCommands::Filters(args) => run_filters(args),
    }
}

fn run_filters(args: FilterVerifyArgs) -> Result<i32> {
    let cwd = crate::cmd_common::caller_cwd()?;
    let root = std::path::PathBuf::from(crate::cmd_common::resolve_path_from_cwd(&args.root, &cwd));
    let results = crate::toml_filters::run_filter_tests(&root, args.filter.as_deref())?;
    let total = results.outcomes.len();
    let passed = results
        .outcomes
        .iter()
        .filter(|outcome| outcome.passed)
        .count();
    let failed = total.saturating_sub(passed);
    let missing = results.filters_without_tests.len();
    let success = failed == 0 && (!args.require_all || missing == 0);

    if args.json {
        let outcomes = results
            .outcomes
            .iter()
            .map(|outcome| {
                json!({
                    "filter": outcome.filter_name,
                    "test": outcome.test_name,
                    "source": outcome.source,
                    "passed": outcome.passed,
                    "expected": outcome.expected,
                    "actual": outcome.actual,
                })
            })
            .collect::<Vec<_>>();
        crate::cmd_common::emit_json(
            &json!({
                "ok": success,
                "total": total,
                "passed": passed,
                "failed": failed,
                "filters_without_tests": results.filters_without_tests,
                "outcomes": outcomes,
            }),
            args.pretty,
        )?;
    } else {
        for outcome in &results.outcomes {
            if !outcome.passed {
                eprintln!(
                    "FAIL [{}] {} ({})\n  expected: {:?}\n  actual:   {:?}",
                    outcome.filter_name,
                    outcome.test_name,
                    outcome.source,
                    outcome.expected,
                    outcome.actual
                );
            }
        }
        if total == 0 {
            println!("No inline filter tests found.");
        } else {
            println!("{passed}/{total} filter tests passed");
        }
        if args.require_all {
            for name in &results.filters_without_tests {
                eprintln!("MISSING tests for filter: {name}");
            }
        }
    }

    if success {
        Ok(0)
    } else {
        Ok(1)
    }
}
