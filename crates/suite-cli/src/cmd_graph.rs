use anyhow::Result;
use clap::{Args, Subcommand};

use crate::memory_store::{add_concept, inspect_graph, link_concepts};

#[derive(Args)]
pub struct GraphArgs {
    #[command(subcommand)]
    pub command: GraphCommands,
}

#[derive(Subcommand)]
pub enum GraphCommands {
    Create(GraphCreateArgs),
    AddConcept(GraphAddConceptArgs),
    Link(GraphLinkArgs),
    Inspect(GraphInspectArgs),
}

#[derive(Args)]
pub struct GraphCreateArgs {
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct GraphAddConceptArgs {
    pub name: String,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct GraphLinkArgs {
    pub source: String,
    pub target: String,
    #[arg(long, default_value = "related_to")]
    pub relation: String,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct GraphInspectArgs {
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

pub fn run(args: GraphArgs) -> Result<i32> {
    match args.command {
        GraphCommands::Create(args) => {
            let graph = inspect_graph(1)?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(graph)?, args.pretty)?;
            } else {
                println!("graph ready");
            }
        }
        GraphCommands::AddConcept(args) => {
            let concept = add_concept(&args.name, args.description.as_deref())?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(concept)?, args.pretty)?;
            } else {
                println!("concept {}", concept.name);
            }
        }
        GraphCommands::Link(args) => {
            let relation = link_concepts(&args.source, &args.target, &args.relation)?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(relation)?, args.pretty)?;
            } else {
                println!(
                    "{} -{}-> {}",
                    relation.source, relation.relation, relation.target
                );
            }
        }
        GraphCommands::Inspect(args) => {
            let graph = inspect_graph(args.limit)?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(graph)?, args.pretty)?;
            } else {
                println!("concepts={}", graph.concepts.len());
                println!("relations={}", graph.relations.len());
            }
        }
    }
    Ok(0)
}
