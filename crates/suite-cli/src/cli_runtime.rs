use std::path::Path;

use anyhow::Result;
use clap::Parser;
use serde_json::{json, Value};

use crate::{
    cmd_agent_prompt, cmd_build, cmd_common, cmd_compact, cmd_context, cmd_cover, cmd_daemon,
    cmd_dashboard, cmd_diff, cmd_discover, cmd_doctor, cmd_feedback, cmd_graph, cmd_guard,
    cmd_hook, cmd_impact, cmd_init, cmd_learn, cmd_map, cmd_map_query, cmd_map_repo, cmd_mcp,
    cmd_memory, cmd_packet, cmd_proxy, cmd_run, cmd_setup, cmd_shard, cmd_shell, cmd_stack,
    cmd_system, cmd_transcript, cmd_verify, cmd_wakeup, BuildCommands, Cli, Commands,
    ContextCommands, CoverCommands, DiffCommands, GuardCommands, MapCommands, StackCommands,
    TestCommands,
};

pub fn main_entry() {
    let raw_args = std::env::args().collect::<Vec<_>>();
    let cli = Cli::parse();
    let machine_error = machine_error_context(&cli);
    if let Err(e) = configure_stdout_output(cli.output.as_deref()) {
        display_error(&e);
        std::process::exit(2);
    }

    let result = run_cli(cli, &raw_args);
    match result {
        Ok(exit_code) => std::process::exit(exit_code),
        Err(e) => {
            if let Some(context) = machine_error {
                if let Err(emit_err) = cmd_common::emit_machine_error(
                    &context.command,
                    &e,
                    context.pretty,
                    context.target.as_deref(),
                    context.retry_hint,
                ) {
                    display_error(&emit_err);
                }
                std::process::exit(2);
            }
            display_error(&e);
            std::process::exit(2);
        }
    }
}

pub fn run_cli(mut cli: Cli, raw_args: &[String]) -> Result<i32> {
    if !matches!(cli.command, Some(Commands::Daemon(_)))
        && (cli.via_daemon || cmd_daemon::via_daemon_env_enabled())
    {
        cli.via_daemon = true;
        return cmd_daemon::run_via_daemon(cli, raw_args);
    }

    run_cli_local(cli)
}

pub fn run_cli_local(cli: Cli) -> Result<i32> {
    match cli.command {
        None => cmd_setup::run(cmd_setup::SetupArgs {
            root: ".".to_string(),
            yes: false,
            fallback_only: false,
            runtime: "all".to_string(),
        }),
        Some(Commands::Cover(cover)) => match cover.command {
            CoverCommands::Check(args) => cmd_cover::run(args, &cli.config),
        },
        Some(Commands::Diff(diff)) => match diff.command {
            DiffCommands::Analyze(args) => cmd_diff::run(args, &cli.config),
        },
        Some(Commands::Test(test)) => match test.command {
            TestCommands::Impact(args) => cmd_impact::run(args, &cli.config),
            TestCommands::Shard(args) => cmd_shard::run(args, &cli.config),
            TestCommands::Map(args) => cmd_map::run(args),
        },
        Some(Commands::Guard(guard)) => match guard.command {
            GuardCommands::Validate(args) => cmd_guard::run_validate(args, &cli.config),
            GuardCommands::Check(args) => cmd_guard::run_check(args, &cli.config),
        },
        Some(Commands::Context(context)) => match context.command {
            ContextCommands::Assemble(args) => cmd_context::run_assemble(args),
            ContextCommands::Correlate(args) => cmd_context::run_correlate(args),
            ContextCommands::Manage(args) => cmd_context::run_manage(args),
            ContextCommands::State(args) => cmd_context::run_state(args),
            ContextCommands::Store(args) => cmd_context::run_store(args),
            ContextCommands::Recall(args) => cmd_context::run_recall(args),
        },
        Some(Commands::Stack(stack)) => match stack.command {
            StackCommands::Slice(args) => cmd_stack::run(args),
        },
        Some(Commands::Build(build)) => match build.command {
            BuildCommands::Reduce(args) => cmd_build::run(args),
        },
        Some(Commands::Map(map)) => match map.command {
            MapCommands::Repo(args) => cmd_map_repo::run(args),
            MapCommands::Query(args) => cmd_map_query::run(args),
        },
        Some(Commands::Proxy(proxy)) => match proxy.command {
            cmd_proxy::ProxyCommands::Run(args) => cmd_proxy::run(args),
        },
        Some(Commands::Gain(args)) => cmd_compact::run_gain_command(args),
        Some(Commands::Compact(args)) => cmd_compact::run(args),
        Some(Commands::Read(args)) => cmd_system::run_read(args),
        Some(Commands::Summary(args)) => cmd_system::run_summary(args),
        Some(Commands::Find(args)) => cmd_system::run_find(args),
        Some(Commands::Grep(args)) => cmd_system::run_grep(args),
        Some(Commands::Json(args)) => cmd_system::run_json(args),
        Some(Commands::Deps(args)) => cmd_system::run_deps(args),
        Some(Commands::Env(args)) => cmd_system::run_env(args),
        Some(Commands::Log(args)) => cmd_system::run_log(args),
        Some(Commands::Rewrite(args)) => cmd_compact::run_rewrite_command(args),
        Some(Commands::Session(args)) => cmd_compact::run_session(args),
        Some(Commands::Packet(packet)) => match packet.command {
            cmd_packet::PacketCommands::Fetch(args) => cmd_packet::run_fetch(args),
        },
        Some(Commands::Memory(args)) => cmd_memory::run(args),
        Some(Commands::Feedback(args)) => cmd_feedback::run(args),
        Some(Commands::Graph(args)) => cmd_graph::run(args),
        Some(Commands::Transcript(args)) => cmd_transcript::run(args),
        Some(Commands::Wakeup(args)) => cmd_wakeup::run(args),
        Some(Commands::Dashboard(args)) => cmd_dashboard::run(args),
        Some(Commands::AgentPrompt(args)) => cmd_agent_prompt::run(args),
        Some(Commands::Mcp(args)) => cmd_mcp::run(args),
        Some(Commands::Hook(args)) => cmd_hook::run(args),
        Some(Commands::Daemon(daemon)) => cmd_daemon::run(daemon),
        Some(Commands::Doctor(args)) => cmd_doctor::run(args),
        Some(Commands::Setup(args)) => cmd_setup::run(args),
        Some(Commands::Init(args)) => cmd_init::run(args),
        Some(Commands::Run(args)) => cmd_run::run(args),
        Some(Commands::Verify(args)) => cmd_verify::run(args),
        Some(Commands::Shell(args)) => cmd_shell::run(args),
        Some(Commands::Discover(args)) => cmd_discover::run(args),
        Some(Commands::Learn(args)) => cmd_learn::run(args),
    }
}

pub fn display_error(err: &anyhow::Error) {
    use colored::Colorize;

    if let Some(covy_err) = err.downcast_ref::<suite_packet_core::CovyError>() {
        eprintln!("{} {covy_err}", "error:".red().bold());
        if let Some(hint) = covy_err.hint() {
            eprintln!("  {} {hint}", "hint:".cyan().bold());
        }
    } else {
        eprintln!("{} {err}", "error:".red().bold());
        for cause in err.chain().skip(1) {
            eprintln!("  {} {cause}", "caused by:".dimmed());
        }
    }
}

struct MachineErrorContext {
    command: String,
    pretty: bool,
    target: Option<String>,
    retry_hint: Option<Value>,
}

fn machine_error_context(cli: &Cli) -> Option<MachineErrorContext> {
    match &cli.command {
        None => None,
        Some(Commands::Cover(cover)) => match &cover.command {
            CoverCommands::Check(args) if args.machine_output_requested() => {
                Some(MachineErrorContext {
                    command: "Packet28 cover check".to_string(),
                    pretty: args.pretty_output(),
                    target: Some("cover.check".to_string()),
                    retry_hint: None,
                })
            }
            _ => None,
        },
        Some(Commands::Diff(diff)) => match &diff.command {
            DiffCommands::Analyze(args) if args.machine_output_requested() => {
                Some(MachineErrorContext {
                    command: "Packet28 diff analyze".to_string(),
                    pretty: args.pretty_output(),
                    target: Some("diffy.analyze".to_string()),
                    retry_hint: governed_retry_hint(
                        args.governed_requested(),
                        "Packet28 diff analyze --context-config <context.yaml>",
                    ),
                })
            }
            _ => None,
        },
        Some(Commands::Test(test)) => match &test.command {
            TestCommands::Impact(args) if args.json.is_some() || args.legacy_json => {
                Some(MachineErrorContext {
                    command: "Packet28 test impact".to_string(),
                    pretty: args.pretty,
                    target: Some("testy.impact".to_string()),
                    retry_hint: governed_retry_hint(
                        args.context_config.is_some(),
                        "Packet28 test impact --context-config <context.yaml>",
                    ),
                })
            }
            TestCommands::Shard(args) if args.json => Some(MachineErrorContext {
                command: "Packet28 test shard".to_string(),
                pretty: false,
                target: Some("testy.shard".to_string()),
                retry_hint: None,
            }),
            TestCommands::Map(args) if args.json => Some(MachineErrorContext {
                command: "Packet28 test map".to_string(),
                pretty: false,
                target: Some("testy.map".to_string()),
                retry_hint: None,
            }),
            _ => None,
        },
        Some(Commands::Context(context)) => match &context.command {
            ContextCommands::Assemble(args) if args.machine_output_requested() => {
                Some(MachineErrorContext {
                    command: "Packet28 context assemble".to_string(),
                    pretty: args.pretty_output(),
                    target: Some(if args.governed_requested() {
                        "governed.assemble".to_string()
                    } else {
                        "contextq.assemble".to_string()
                    }),
                    retry_hint: governed_retry_hint(
                        args.governed_requested(),
                        "Packet28 context assemble --context-config <context.yaml>",
                    ),
                })
            }
            ContextCommands::Correlate(args) if args.machine_output_requested() => {
                Some(MachineErrorContext {
                    command: "Packet28 context correlate".to_string(),
                    pretty: args.pretty_output(),
                    target: Some("contextq.correlate".to_string()),
                    retry_hint: None,
                })
            }
            ContextCommands::Manage(args) if args.machine_output_requested() => {
                Some(MachineErrorContext {
                    command: "Packet28 context manage".to_string(),
                    pretty: args.pretty_output(),
                    target: Some("contextq.manage".to_string()),
                    retry_hint: None,
                })
            }
            ContextCommands::State(state) => match &state.command {
                cmd_context::StateCommands::Append(args) if args.machine_output_requested() => {
                    Some(MachineErrorContext {
                        command: "Packet28 context state append".to_string(),
                        pretty: args.pretty_output(),
                        target: Some("agenty.state.write".to_string()),
                        retry_hint: None,
                    })
                }
                cmd_context::StateCommands::Snapshot(args) if args.machine_output_requested() => {
                    Some(MachineErrorContext {
                        command: "Packet28 context state snapshot".to_string(),
                        pretty: args.pretty_output(),
                        target: Some("agenty.state.snapshot".to_string()),
                        retry_hint: None,
                    })
                }
                _ => None,
            },
            ContextCommands::Store(store) => match &store.command {
                cmd_context::StoreCommands::List(args) if args.machine_output_requested() => {
                    Some(machine_error(
                        "Packet28 context store list",
                        args.pretty_output(),
                        "context.store.list",
                    ))
                }
                cmd_context::StoreCommands::Get(args) if args.machine_output_requested() => {
                    Some(machine_error(
                        "Packet28 context store get",
                        args.pretty_output(),
                        "context.store.get",
                    ))
                }
                cmd_context::StoreCommands::Prune(args) if args.machine_output_requested() => {
                    Some(machine_error(
                        "Packet28 context store prune",
                        args.pretty_output(),
                        "context.store.prune",
                    ))
                }
                cmd_context::StoreCommands::Stats(args) if args.machine_output_requested() => {
                    Some(machine_error(
                        "Packet28 context store stats",
                        args.pretty_output(),
                        "context.store.stats",
                    ))
                }
                _ => None,
            },
            ContextCommands::Recall(args) if args.machine_output_requested() => {
                Some(machine_error(
                    "Packet28 context recall",
                    args.pretty_output(),
                    "context.recall",
                ))
            }
            _ => None,
        },
        Some(Commands::Stack(stack)) => match &stack.command {
            StackCommands::Slice(args) if args.machine_output_requested() => {
                Some(MachineErrorContext {
                    command: "Packet28 stack slice".to_string(),
                    pretty: args.pretty_output(),
                    target: Some("stacky.slice".to_string()),
                    retry_hint: governed_retry_hint(
                        args.governed_requested(),
                        "Packet28 stack slice --context-config <context.yaml>",
                    ),
                })
            }
            _ => None,
        },
        Some(Commands::Build(build)) => match &build.command {
            BuildCommands::Reduce(args) if args.machine_output_requested() => {
                Some(MachineErrorContext {
                    command: "Packet28 build reduce".to_string(),
                    pretty: args.pretty_output(),
                    target: Some("buildy.reduce".to_string()),
                    retry_hint: governed_retry_hint(
                        args.governed_requested(),
                        "Packet28 build reduce --context-config <context.yaml>",
                    ),
                })
            }
            _ => None,
        },
        Some(Commands::Map(map)) => match &map.command {
            MapCommands::Repo(args) if args.json.is_some() || args.legacy_json => {
                Some(MachineErrorContext {
                    command: "Packet28 map repo".to_string(),
                    pretty: args.pretty,
                    target: Some("mapy.repo".to_string()),
                    retry_hint: governed_retry_hint(
                        args.context_config.is_some(),
                        "Packet28 map repo --context-config <context.yaml>",
                    ),
                })
            }
            MapCommands::Query(args) if args.json.is_some() || args.legacy_json => {
                Some(MachineErrorContext {
                    command: "Packet28 map query".to_string(),
                    pretty: args.pretty,
                    target: Some("mapy.query".to_string()),
                    retry_hint: None,
                })
            }
            _ => None,
        },
        Some(Commands::Proxy(proxy)) => match &proxy.command {
            cmd_proxy::ProxyCommands::Run(args) if args.json.is_some() || args.legacy_json => {
                Some(MachineErrorContext {
                    command: "Packet28 proxy run".to_string(),
                    pretty: args.pretty,
                    target: Some("proxy.run".to_string()),
                    retry_hint: governed_retry_hint(
                        args.context_config.is_some(),
                        "Packet28 proxy run --context-config <context.yaml> -- <command>",
                    ),
                })
            }
            _ => None,
        },
        Some(Commands::Packet(packet)) => {
            match &packet.command {
                cmd_packet::PacketCommands::Fetch(args) if args.json.is_some() => Some(
                    machine_error("Packet28 packet fetch", args.pretty, "packet.fetch"),
                ),
                _ => None,
            }
        }
        Some(Commands::Gain(_))
        | Some(Commands::Compact(_))
        | Some(Commands::Read(_))
        | Some(Commands::Summary(_))
        | Some(Commands::Find(_))
        | Some(Commands::Grep(_))
        | Some(Commands::Json(_))
        | Some(Commands::Deps(_))
        | Some(Commands::Env(_))
        | Some(Commands::Log(_))
        | Some(Commands::Rewrite(_))
        | Some(Commands::Session(_))
        | Some(Commands::Discover(_))
        | Some(Commands::Learn(_)) => None,
        Some(Commands::Daemon(_))
        | Some(Commands::Guard(_))
        | Some(Commands::Memory(_))
        | Some(Commands::Feedback(_))
        | Some(Commands::Graph(_))
        | Some(Commands::Transcript(_))
        | Some(Commands::Wakeup(_))
        | Some(Commands::Dashboard(_))
        | Some(Commands::AgentPrompt(_))
        | Some(Commands::Mcp(_))
        | Some(Commands::Hook(_))
        | Some(Commands::Setup(_))
        | Some(Commands::Init(_))
        | Some(Commands::Run(_))
        | Some(Commands::Verify(_))
        | Some(Commands::Shell(_))
        | Some(Commands::Doctor(_)) => None,
    }
}

fn machine_error(command: &str, pretty: bool, target: &str) -> MachineErrorContext {
    MachineErrorContext {
        command: command.to_string(),
        pretty,
        target: Some(target.to_string()),
        retry_hint: None,
    }
}

fn governed_retry_hint(enabled: bool, command: &str) -> Option<Value> {
    enabled.then(|| {
        json!({
            "retry_command": command
        })
    })
}

#[cfg(unix)]
fn configure_stdout_output(path: Option<&str>) -> anyhow::Result<()> {
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd;

    let Some(path) = path else {
        return Ok(());
    };

    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(Path::new(path))?;

    let ret = unsafe { libc::dup2(file.as_raw_fd(), libc::STDOUT_FILENO) };
    if ret < 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    Ok(())
}

#[cfg(not(unix))]
fn configure_stdout_output(path: Option<&str>) -> anyhow::Result<()> {
    if path.is_some() {
        anyhow::bail!("--output is currently supported only on Unix targets");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_subcommand_defaults_to_setup_wizard() {
        let cli = Cli::try_parse_from(["Packet28"]).unwrap();

        assert!(cli.command.is_none());
        assert!(machine_error_context(&cli).is_none());
    }

    #[test]
    fn explicit_subcommands_still_parse() {
        let cli = Cli::try_parse_from(["Packet28", "doctor", "--root", "."]).unwrap();

        assert!(matches!(cli.command, Some(Commands::Doctor(_))));
    }
}
