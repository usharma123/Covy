use anyhow::Result;
use clap::Args;

#[derive(Args)]
pub struct DigestArgs {
    #[arg(long, default_value = ".")]
    pub root: String,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

pub fn run(args: DigestArgs) -> Result<i32> {
    let root = crate::cmd_daemon::resolve_root_arg(&args.root);
    let digest = crate::cmd_dashboard::context_anomaly_digest(&root)?;
    if args.json {
        if args.pretty {
            println!("{}", serde_json::to_string_pretty(&digest)?);
        } else {
            println!("{}", serde_json::to_string(&digest)?);
        }
        return Ok(0);
    }

    println!("anomaly_count={}", digest.anomaly_count);
    for anomaly in digest.anomalies {
        println!(
            "anomaly category={} severity={} signal={} next_check={} repair_hint={}",
            anomaly.category,
            anomaly.severity,
            anomaly.signal,
            anomaly.next_check,
            anomaly.repair_hint
        );
    }
    Ok(0)
}
