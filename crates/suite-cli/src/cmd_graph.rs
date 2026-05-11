use anyhow::Result;
use clap::{Args, Subcommand};

use crate::memory_store::{
    add_concept_with_metadata, create_graph_memoir, delete_concept, export_graph, graph_stats,
    inspect_graph, link_concepts, list_graph_memoirs, refine_concept, search_concepts,
    show_graph_memoir,
};

#[derive(Args)]
pub struct GraphArgs {
    #[command(subcommand)]
    pub command: GraphCommands,
}

#[derive(Subcommand)]
pub enum GraphCommands {
    Create(GraphCreateArgs),
    List(GraphListArgs),
    Show(GraphShowArgs),
    AddConcept(GraphAddConceptArgs),
    Refine(GraphRefineArgs),
    Delete(GraphDeleteArgs),
    Search(GraphSearchArgs),
    Export(GraphExportArgs),
    Stats(GraphStatsArgs),
    Link(GraphLinkArgs),
    Inspect(GraphInspectArgs),
}

#[derive(Args)]
pub struct GraphCreateArgs {
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct GraphListArgs {
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct GraphShowArgs {
    #[arg(default_value = "default")]
    pub name: String,
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
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
    pub memoir: Option<String>,
    #[arg(long = "label")]
    pub labels: Vec<String>,
    #[arg(long)]
    pub confidence: Option<f64>,
    #[arg(long = "source-id")]
    pub source_ids: Vec<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct GraphRefineArgs {
    pub name: String,
    pub description: String,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct GraphDeleteArgs {
    pub name: String,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct GraphSearchArgs {
    pub query: String,
    #[arg(long, default_value_t = 10)]
    pub limit: usize,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct GraphExportArgs {
    #[arg(long, default_value = "json")]
    pub format: String,
    #[arg(long, default_value_t = 100)]
    pub limit: usize,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct GraphStatsArgs {
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
            let memoir = create_graph_memoir(args.name.as_deref(), args.description.as_deref())?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(memoir)?, args.pretty)?;
            } else {
                println!("memoir {}", memoir.name);
            }
        }
        GraphCommands::List(args) => {
            let memoirs = list_graph_memoirs()?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(memoirs)?, args.pretty)?;
            } else {
                for memoir in memoirs {
                    println!(
                        "{} concepts={} relations={} avg_confidence={:.2}",
                        memoir.name,
                        memoir.concept_count,
                        memoir.relation_count,
                        memoir.average_confidence
                    );
                }
            }
        }
        GraphCommands::Show(args) => {
            let memoir = show_graph_memoir(Some(&args.name), args.limit)?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(memoir)?, args.pretty)?;
            } else {
                println!("memoir {}", memoir.memoir.name);
                println!("concepts={}", memoir.concepts.len());
                println!("relations={}", memoir.relations.len());
            }
        }
        GraphCommands::AddConcept(args) => {
            let concept = add_concept_with_metadata(
                &args.name,
                args.description.as_deref(),
                args.memoir.as_deref(),
                &args.labels,
                args.confidence,
                &args.source_ids,
            )?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(concept)?, args.pretty)?;
            } else {
                println!("concept {}", concept.name);
            }
        }
        GraphCommands::Refine(args) => {
            let concept = refine_concept(&args.name, &args.description)?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(concept)?, args.pretty)?;
            } else {
                println!("concept {}", concept.name);
            }
        }
        GraphCommands::Delete(args) => {
            let report = delete_concept(&args.name)?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(report)?, args.pretty)?;
            } else {
                println!("deleted_concepts={}", report.deleted_concepts);
                println!("deleted_relations={}", report.deleted_relations);
            }
        }
        GraphCommands::Search(args) => {
            let concepts = search_concepts(&args.query, args.limit)?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(concepts)?, args.pretty)?;
            } else {
                for concept in concepts {
                    println!("{} {}", concept.id, concept.name);
                }
            }
        }
        GraphCommands::Export(args) => {
            let export = export_graph(&args.format, args.limit)?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(export)?, args.pretty)?;
            } else {
                print!("{}", export.content);
            }
        }
        GraphCommands::Stats(args) => {
            let stats = graph_stats()?;
            if args.json {
                crate::cmd_common::emit_json(&serde_json::to_value(stats)?, args.pretty)?;
            } else {
                println!("concept_count={}", stats.concept_count);
                println!("relation_count={}", stats.relation_count);
                println!("relation_type_count={}", stats.relation_type_count);
                println!("isolated_concept_count={}", stats.isolated_concept_count);
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
