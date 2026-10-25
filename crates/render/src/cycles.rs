//! The import cycles' two forms: arrow chains in the text, and every cycle's files in the JSON.

use app::{CycleReport, CycleRow};
use prompts::messages::{CYCLES_HEADER, CYCLES_UNLISTED, CYCLES_UNRESOLVED, GROUPED_FILES};
use serde::Serialize;
use text::one_line;

use crate::envelope::{Listing, wrap_json};
use crate::skipped::{SkippedJson, skipped_json, skipped_line};
use crate::{Rendered, View};

// The text names at most this many off-chain members of one cycle.
const CYCLE_MEMBERS_SHOWN: usize = 10;

impl View for CycleReport {
    fn rendered(&self) -> Rendered<'_> {
        Rendered::new(cycles_text(self), CyclesJson::of(self), None, wrap_json)
    }
}

// Keyed on the total, not on the rows the limit kept, so a limit never reads as "no cycles".
fn cycles_text(report: &CycleReport) -> String {
    let total = report.cycles.total();
    let mut lines = Vec::new();
    if total == 0 {
        lines.push("no import cycles".to_owned());
        lines.extend(unresolved_note(report.unresolved_internal_imports));
    } else {
        lines.push(CYCLES_HEADER.render(total));
        lines.extend(unresolved_note(report.unresolved_internal_imports));
        for (index, cycle) in report.cycles.rows().iter().enumerate() {
            lines.extend(cycle_lines(index + 1, cycle));
        }
        if report.cycles.omitted() > 0 {
            let limit = report.cycles.limit();
            lines.push(CYCLES_UNLISTED.render((report.cycles.omitted(), limit)));
        }
    }
    let body = lines.join("\n") + "\n";
    let warning = skipped_line(&report.skipped);
    if warning.is_empty() {
        body
    } else {
        format!("{warning}\n\n{body}")
    }
}

fn cycle_lines(number: usize, cycle: &CycleRow) -> Vec<String> {
    let component = component(cycle);
    let chain = one_line(&chain(cycle)).into_owned();
    let size = GROUPED_FILES.render(component.len());
    let mut lines = vec![format!("  {number:>3}. {size} {chain}")];
    let off_chain: Vec<&str> = component
        .into_iter()
        .filter(|path| !cycle.files.iter().any(|file| file == path))
        .collect();
    if !off_chain.is_empty() {
        lines.push(format!(
            "         also in this cycle: {}",
            listed_members(&off_chain)
        ));
    }
    lines
}

fn listed_members(paths: &[&str]) -> String {
    let shown: Vec<String> = paths
        .iter()
        .take(CYCLE_MEMBERS_SHOWN)
        .map(|path| one_line(path).into_owned())
        .collect();
    let hidden = paths.len().saturating_sub(CYCLE_MEMBERS_SHOWN);
    if hidden > 0 {
        format!("{}; ... {hidden} more", shown.join(", "))
    } else {
        shown.join(", ")
    }
}

// Only an unresolved import of the repository's own modules can hide a cycle.
fn unresolved_note(count: u32) -> Option<String> {
    (count > 0).then(|| CYCLES_UNRESOLVED.render(count as usize))
}

// The chain closes the ring by naming its first file again.
fn chain(cycle: &CycleRow) -> String {
    let first = cycle.files.first().map(String::as_str);
    let ring: Vec<&str> = cycle
        .files
        .iter()
        .map(String::as_str)
        .chain(first)
        .collect();
    ring.join(" -> ")
}

// Every file of the cycle, sorted: the members, or the chain's own files without them.
fn component(cycle: &CycleRow) -> Vec<&str> {
    if !cycle.members.is_empty() {
        return cycle.members.iter().map(String::as_str).collect();
    }
    let mut files: Vec<&str> = cycle.files.iter().map(String::as_str).collect();
    files.sort_unstable();
    files
}

#[derive(Clone, Serialize)]
struct CyclesJson<'a> {
    total: usize,
    limit: usize,
    omitted: usize,
    unresolved_imports: u32,
    unresolved_internal_imports: u32,
    skipped: Vec<SkippedJson<'a>>,
    cycles: Vec<CycleJson<'a>>,
}

#[derive(Clone, Serialize)]
struct CycleJson<'a> {
    length: usize,
    files: &'a [String],
    chain: String,
    members: Vec<&'a str>,
}

impl<'a> CyclesJson<'a> {
    fn of(report: &'a CycleReport) -> Self {
        let cycles = report.cycles.rows().iter().map(|cycle| CycleJson {
            length: cycle.files.len(),
            files: &cycle.files,
            chain: chain(cycle),
            members: component(cycle),
        });
        CyclesJson {
            total: report.cycles.total(),
            limit: report.cycles.limit(),
            omitted: report.cycles.omitted(),
            unresolved_imports: report.unresolved_imports,
            unresolved_internal_imports: report.unresolved_internal_imports,
            skipped: skipped_json(&report.skipped),
            cycles: cycles.collect(),
        }
    }
}

impl Listing for CyclesJson<'_> {
    fn items(&self) -> usize {
        self.cycles.len()
    }

    fn prefix(&self, kept: usize) -> Self {
        let mut prefix = self.clone();
        prefix.cycles.truncate(kept);
        prefix
    }
}
