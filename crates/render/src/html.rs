//! The HTML export: one self-contained document whose graph data is one JSON value that no
//! repository text can break out of, and the receipt summary that names each bound's cut.

mod template;

use std::collections::BTreeMap;

use app::HtmlGraph;
use json::{JsonForm, to_text};
use serde::Serialize;

use crate::Error;

const DATA_MARKER: &str = "__GRAPH_DATA__";

/// Returns the HTML document of `graph`.
///
/// # Errors
///
/// Returns the serializer's error for a value it cannot write.
pub fn html_document(graph: &HtmlGraph) -> Result<String, Error> {
    Ok(template::DOCUMENT.replace(DATA_MARKER, &script_json(&Payload::of(graph))?))
}

/// Returns the one-line summary that the export's receipt carries: what each bound left out.
#[must_use]
pub fn html_summary(graph: &HtmlGraph) -> String {
    format!(
        "HTML graph of {} modules and {} edges; {} modules elided (node bound {}); {} edges \
         elided with them, {} past the edge bound ({})",
        graph.nodes.len(),
        graph.edges.len(),
        graph.elided_nodes,
        graph.node_bound,
        graph.edges_without_both_nodes,
        graph.edges_over_bound,
        graph.edge_bound
    )
}

// Sorted keys and no spaces, with the three characters that could close the script element
// written as escapes.
fn script_json(payload: &Payload<'_>) -> Result<String, Error> {
    let packed = to_text(payload, JsonForm::Packed)?;
    Ok(packed
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Payload<'a> {
    nodes: Vec<NodeJson<'a>>,
    edges: Vec<EdgeJson<'a>>,
    communities: BTreeMap<String, &'a str>,
    elided_nodes: usize,
    edges_without_both_nodes: usize,
    edges_over_bound: usize,
    node_bound: usize,
    edge_bound: usize,
}

#[derive(Serialize)]
struct NodeJson<'a> {
    id: &'a str,
    path: &'a str,
    community: usize,
}

#[derive(Serialize)]
struct EdgeJson<'a> {
    source: &'a str,
    target: &'a str,
    kind: &'a str,
    weight: f64,
}

impl<'a> Payload<'a> {
    fn of(graph: &'a HtmlGraph) -> Self {
        let nodes = graph.nodes.iter().map(|node| NodeJson {
            id: &node.id,
            path: &node.path,
            community: node.community,
        });
        let edges = graph.edges.iter().map(|edge| EdgeJson {
            source: &edge.source,
            target: &edge.target,
            kind: if edge.import { "import" } else { "reference" },
            weight: edge.weight,
        });
        let communities = graph.communities.iter();
        Payload {
            nodes: nodes.collect(),
            edges: edges.collect(),
            communities: communities
                .map(|(at, label)| (at.to_string(), label.as_str()))
                .collect(),
            elided_nodes: graph.elided_nodes,
            edges_without_both_nodes: graph.edges_without_both_nodes,
            edges_over_bound: graph.edges_over_bound,
            node_bound: graph.node_bound,
            edge_bound: graph.edge_bound,
        }
    }
}
