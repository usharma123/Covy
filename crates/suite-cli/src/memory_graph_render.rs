use crate::memory_store_types::GraphInspect;

pub(crate) fn render_graph_dot(graph: &GraphInspect) -> String {
    let mut out = String::from("digraph packet28_graph {\n");
    for concept in &graph.concepts {
        out.push_str(&format!("  {:?};\n", concept.name));
    }
    for relation in &graph.relations {
        out.push_str(&format!(
            "  {:?} -> {:?} [label={:?}];\n",
            relation.source, relation.target, relation.relation
        ));
    }
    out.push_str("}\n");
    out
}

pub(crate) fn render_graph_ascii(graph: &GraphInspect) -> String {
    let mut out = String::new();
    for concept in &graph.concepts {
        out.push_str(&format!("* {}\n", concept.name));
        if let Some(description) = &concept.description {
            out.push_str(&format!("  {description}\n"));
        }
    }
    for relation in &graph.relations {
        out.push_str(&format!(
            "{} -{}-> {}\n",
            relation.source, relation.relation, relation.target
        ));
    }
    out
}
