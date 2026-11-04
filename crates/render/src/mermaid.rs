//! The diagram's forms: bare mermaid text for a document, the fenced text with its caveat for a
//! response body, the whole view as JSON, and the receipt summary and caveat of the command line.

use app::{ChartNode, DiagramView, Flowchart};
use prompts::envelope::RECEIPT_NOTE;
use prompts::messages::{
    DIAGRAM_EDGES_UNLISTED, DIAGRAM_ELIDED, DIAGRAM_NOT_CONVERGED, DIAGRAM_SUMMARY,
    DIAGRAM_SUMMARY_FOCUS, DIAGRAM_SUMMARY_FOCUS_GROUPED, DIAGRAM_SUMMARY_GROUPED, DIAGRAM_TITLES,
};
use serde::Serialize;
use text::one_line;

use crate::envelope::wrap_json_whole;
use crate::{Rendered, View};

const INDENT: &str = "    ";
const FENCE: &str = "```";
const EDGE_LEGEND: &str = "%% solid: imports, dashed: references";
const ELISION_ID: &str = "n_elided";

impl View for DiagramView {
    fn rendered(&self) -> Rendered<'_> {
        Rendered::new(fenced(self), DiagramJson::of(self), None, wrap_json_whole)
    }
}

/// Returns the view's flowchart as bare mermaid text, or an empty text when it has none.
#[must_use]
pub fn mermaid(view: &DiagramView) -> String {
    let Some(chart) = &view.chart else {
        return String::new();
    };
    let mut lines = vec!["flowchart LR".to_owned()];
    // Keyed on the arrows drawn: a legend above a diagram that draws neither style misleads.
    if !chart.edges.is_empty() {
        lines.push(format!("{INDENT}{EDGE_LEGEND}"));
    }
    lines.extend(node_lines(chart));
    lines.extend(edge_lines(view, chart));
    if view.elided > 0 {
        let elided = DIAGRAM_ELIDED.render(view.elided);
        lines.push(format!("{INDENT}{ELISION_ID}[\"{elided}\"]"));
    }
    lines.join("\n") + "\n"
}

/// Returns the one-line summary that the diagram's receipt carries.
#[must_use]
pub fn diagram_summary(view: &DiagramView) -> String {
    let counts = (view.nodes, view.elided);
    match (view.focus.is_empty(), view.grouped) {
        (true, false) => DIAGRAM_SUMMARY.render(counts),
        (true, true) => DIAGRAM_SUMMARY_GROUPED.render(counts),
        (false, false) => DIAGRAM_SUMMARY_FOCUS.render((&view.focus, counts.0, counts.1)),
        (false, true) => DIAGRAM_SUMMARY_FOCUS_GROUPED.render((&view.focus, counts.0, counts.1)),
    }
}

/// Returns the qualifications that a bounded diagram carries, one note line each, or `None`.
#[must_use]
pub fn diagram_caveat(view: &DiagramView) -> Option<String> {
    let notes = caveat_notes(view);
    let lines: Vec<String> = notes.iter().map(|note| RECEIPT_NOTE.render(note)).collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

// The text marks each note as the receipt marks its note; the JSON carries the notes unmarked.
fn caveat_notes(view: &DiagramView) -> Vec<String> {
    let mut notes = Vec::new();
    if !view.rank_converged {
        notes.push(DIAGRAM_NOT_CONVERGED.to_owned());
    }
    // A subgraph is titled after its whole community, including members the bound left out.
    if view.grouped && view.elided > 0 {
        notes.push(DIAGRAM_TITLES.render(view.elided));
    }
    notes
}

/// Returns the diagram inside a markdown fence that opens and closes `text`, or `text`.
#[must_use]
pub fn strip_fence(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let blank = |line: &&str| text::strip(line).is_empty();
    let Some(first) = lines.iter().position(|line| !blank(line)) else {
        return text.to_owned();
    };
    if !lines[first].starts_with(FENCE) {
        return text.to_owned();
    }
    let rest = &lines[first..];
    let closing = rest.iter().rposition(|line| !blank(line)).unwrap_or(0);
    if closing == 0 || text::strip(rest[closing]) != FENCE {
        return text.to_owned();
    }
    rest[1..closing].join("\n") + "\n"
}

fn node_lines(chart: &Flowchart) -> Vec<String> {
    let mut lines = Vec::new();
    for group in &chart.groups {
        let title = &group.title;
        lines.push(format!("{INDENT}subgraph {}[\"{title}\"]", group.id));
        let members = group.members.iter();
        lines.extend(members.map(|member| declaration(member, &INDENT.repeat(2))));
        lines.push(format!("{INDENT}end"));
    }
    lines.extend(chart.nodes.iter().map(|node| declaration(node, INDENT)));
    lines
}

fn declaration(node: &ChartNode, indent: &str) -> String {
    format!("{indent}{}[\"{}\"]", node.id, node.label)
}

// Declared imports draw solid and references dashed; references past the bound are counted in a
// comment instead.
fn edge_lines(view: &DiagramView, chart: &Flowchart) -> Vec<String> {
    let mut lines: Vec<String> = (chart.edges.iter())
        .map(|edge| {
            let arrow = if edge.solid { "-->" } else { "-.->" };
            format!("{INDENT}{} {arrow} {}", edge.from, edge.to)
        })
        .collect();
    if view.edges_over_bound > 0 {
        let unlisted = DIAGRAM_EDGES_UNLISTED.render((view.edges_over_bound, chart.edge_bound));
        lines.push(format!("{INDENT}{unlisted}"));
    }
    lines
}

// The response-body form: the fenced flowchart with its caveat, or the message.
fn fenced(view: &DiagramView) -> String {
    let text = mermaid(view);
    if text.is_empty() {
        let message = if view.message.is_empty() {
            "no diagram"
        } else {
            &view.message
        };
        return one_line(message).trim_end_matches('\n').to_owned() + "\n";
    }
    let body = format!("{FENCE}mermaid\n{}\n{FENCE}\n", text.trim_end_matches('\n'));
    match diagram_caveat(view) {
        Some(caveat) => format!("{body}\n{caveat}\n"),
        None => body,
    }
}

#[derive(Serialize)]
struct DiagramJson<'a> {
    mermaid: String,
    nodes: usize,
    elided: usize,
    edges_over_bound: usize,
    grouped: bool,
    focus: &'a str,
    message: &'a str,
    caveat: String,
    rank_converged: bool,
}

impl<'a> DiagramJson<'a> {
    fn of(view: &'a DiagramView) -> Self {
        DiagramJson {
            mermaid: mermaid(view),
            nodes: view.nodes,
            elided: view.elided,
            edges_over_bound: view.edges_over_bound,
            grouped: view.grouped,
            focus: &view.focus,
            message: &view.message,
            caveat: caveat_notes(view).join("\n"),
            rank_converged: view.rank_converged,
        }
    }
}
