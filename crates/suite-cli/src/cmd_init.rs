use anyhow::Result;
use clap::Args;

use crate::cmd_setup::SetupArgs;

#[derive(Args)]
pub struct InitArgs {
    /// Workspace root for Packet28
    #[arg(long, default_value = ".")]
    pub root: String,

    /// Agent runtime to configure (claude, cursor, codex, windsurf, all)
    #[arg(long, default_value = "all")]
    pub agent: String,

    /// Skip interactive prompts and apply the selected setup
    #[arg(long)]
    pub yes: bool,
}

pub fn run(args: InitArgs) -> Result<i32> {
    crate::cmd_setup::run(SetupArgs {
        root: args.root,
        yes: args.yes,
        fallback_only: false,
        runtime: args.agent,
    })
}
