use std::fs;

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};
use packet28_daemon_core::{BrokerPlanStep, BrokerValidatePlanRequest, BrokerValidatePlanResponse};

#[derive(Args)]
pub struct PlanArgs {
    #[command(subcommand)]
    pub command: PlanCommands,
}

#[derive(Subcommand)]
pub enum PlanCommands {
    /// Validate an agent implementation plan against broker evidence
    Validate(PlanValidateArgs),
}

#[derive(Args)]
pub struct PlanValidateArgs {
    #[arg(long, default_value = ".")]
    pub root: String,

    #[arg(long)]
    pub task_id: String,

    /// JSON array of BrokerPlanStep objects
    #[arg(long, conflicts_with = "steps_file")]
    pub steps: Option<String>,

    /// Path to a JSON array of BrokerPlanStep objects
    #[arg(long = "steps-file", conflicts_with = "steps")]
    pub steps_file: Option<String>,

    #[arg(long)]
    pub no_read_before_edit: bool,

    #[arg(long)]
    pub no_test_gate: bool,

    #[arg(long)]
    pub budget_tokens: Option<u64>,

    #[arg(long)]
    pub json: bool,

    #[arg(long)]
    pub pretty: bool,
}

pub fn run(args: PlanArgs) -> Result<i32> {
    match args.command {
        PlanCommands::Validate(args) => validate(args),
    }
}

fn validate(args: PlanValidateArgs) -> Result<i32> {
    let root = crate::cmd_daemon::resolve_root_arg(&args.root);
    let steps = load_steps(&args)?;
    let response = crate::broker_client::validate_plan(
        &root,
        BrokerValidatePlanRequest {
            task_id: args.task_id,
            steps,
            require_read_before_edit: Some(!args.no_read_before_edit),
            require_test_gate: Some(!args.no_test_gate),
            budget_tokens: args.budget_tokens,
        },
    )?;
    emit_response(&response, args.json, args.pretty)
}

fn load_steps(args: &PlanValidateArgs) -> Result<Vec<BrokerPlanStep>> {
    let raw = if let Some(steps) = args.steps.as_deref() {
        steps.to_string()
    } else if let Some(path) = args.steps_file.as_deref() {
        fs::read_to_string(path).with_context(|| format!("failed to read steps file '{path}'"))?
    } else {
        return Err(anyhow!("plan validate requires --steps or --steps-file"));
    };
    let steps: Vec<BrokerPlanStep> =
        serde_json::from_str(&raw).context("failed to parse plan steps JSON")?;
    if steps.is_empty() {
        return Err(anyhow!("plan validate requires at least one step"));
    }
    Ok(steps)
}

fn emit_response(response: &BrokerValidatePlanResponse, json: bool, pretty: bool) -> Result<i32> {
    if json {
        crate::cmd_common::emit_json(&serde_json::to_value(response)?, pretty)?;
    } else {
        println!(
            "valid={} test_gate_score={} violations={} warnings={} est_plan_tokens={}",
            response.valid,
            response
                .test_gate_score
                .map(|score| score.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            response.violations.len(),
            response.warnings.len(),
            response
                .est_plan_tokens
                .map(|tokens| tokens.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
        );
        for violation in &response.violations {
            println!(
                "error {} {} {}",
                violation.step_id, violation.rule, violation.message
            );
        }
        for warning in &response.warnings {
            println!(
                "warning {} {} {}",
                warning.step_id, warning.rule, warning.message
            );
        }
    }
    Ok(0)
}
