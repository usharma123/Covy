use anyhow::{bail, Result};
use clap::Args;

use crate::cmd_setup::SetupArgs;

#[derive(Args)]
pub struct InitArgs {
    /// Workspace root for Packet28
    #[arg(long, default_value = ".")]
    pub root: String,

    /// Agent runtime to configure (claude, cursor, codex, windsurf, all)
    #[arg(long)]
    pub agent: Option<String>,

    /// Compatibility alias for selecting an init mode/runtime (claude, cursor, codex, windsurf, all)
    #[arg(long)]
    pub mode: Option<String>,

    /// Skip interactive prompts and apply the selected setup
    #[arg(long)]
    pub yes: bool,
}

pub fn run(args: InitArgs) -> Result<i32> {
    let runtime = resolve_runtime(args.agent, args.mode)?;

    crate::cmd_setup::run(SetupArgs {
        root: args.root,
        yes: args.yes,
        fallback_only: false,
        runtime,
    })
}

fn resolve_runtime(agent: Option<String>, mode: Option<String>) -> Result<String> {
    let runtime = match (agent, mode) {
        (Some(agent), Some(mode)) if agent != mode => {
            bail!("--agent and --mode must match when both are provided")
        }
        (Some(agent), _) => agent,
        (_, Some(mode)) => mode,
        (None, None) => "all".to_string(),
    };

    Ok(runtime)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Args as ClapArgs, Command, FromArgMatches};

    #[test]
    fn init_accepts_mode_alias() {
        let command = InitArgs::augment_args(Command::new("init"));
        let matches = command
            .try_get_matches_from(["init", "--mode", "all", "--yes"])
            .unwrap();
        let args = InitArgs::from_arg_matches(&matches).unwrap();

        assert_eq!(args.mode.as_deref(), Some("all"));
        assert_eq!(resolve_runtime(args.agent, args.mode).unwrap(), "all");
    }

    #[test]
    fn init_rejects_conflicting_agent_and_mode() {
        let err = resolve_runtime(Some("windsurf".to_string()), Some("all".to_string()))
            .expect_err("conflicting init selectors should fail");

        assert!(err.to_string().contains("--agent and --mode must match"));
    }
}
