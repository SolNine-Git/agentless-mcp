//! The map's two forms: the ranked rows under the notes that change how to read them, with the
//! test companions at the foot; and the JSON with the counts behind the rows.

use app::{
    Companion, Companions, MapEntry, MapFile, MapResult, RationaleNode, map_rows_text, omitted_line,
};
use json::round_half_even;
use prompts::messages::MAP_UNRESOLVED_SEEDS;
use serde::Serialize;
use text::one_line;

use crate::envelope::{Listing, wrap_json};
use crate::skipped::{SkippedJson, skipped_json, skipped_line};
use crate::{Rendered, Truncation, View};

const NOT_CONVERGED: &str = "the ranking behind this map did not converge, so the order of the files below is a partial \
     answer";
const WALK_CAPPED: &str = "  note: the walk out to these tests hit its node bound before it \
                           finished, so this section is a floor rather than every test";

impl View for MapResult {
    fn rendered(&self) -> Rendered<'_> {
        let truncation = Truncation {
            shown: self.included,
            total: self.candidates,
            unit: "symbols",
        };
        Rendered::new(
            map_text(self),
            MapJson::of(self),
            Some(truncation),
            wrap_json,
        )
    }
}

// The notes go above the rows: each changes how the ranking below it reads.
pub(crate) fn map_text(result: &MapResult) -> String {
    let mut body = map_rows_text(&result.files);
    let companions = companions_text(&result.companions);
    if !companions.is_empty() {
        body = format!("{body}\n{companions}");
    }
    let notes = notes(result);
    if notes.is_empty() {
        body
    } else {
        notes.join("\n") + "\n\n" + &body
    }
}

fn notes(result: &MapResult) -> Vec<String> {
    let mut notes = Vec::new();
    if !result.rank_converged {
        notes.push(NOT_CONVERGED.to_owned());
    }
    if result.files.is_empty() {
        notes.push(why_nothing_ranked(result));
    }
    if result.budget > 0 && result.rendered > result.budget {
        notes.push(format!(
            "this map renders to {} tokens against a {}-token budget: every one of the {} ranked \
             files is listed whatever the budget allows, and the budget bounds only the symbols \
             shown under them",
            result.rendered,
            result.budget,
            result.files.len()
        ));
    }
    if !result.unresolved_seeds.is_empty() {
        notes.push(MAP_UNRESOLVED_SEEDS.render(&result.unresolved_seeds.join(", ")));
    }
    let warning = skipped_line(&result.skipped);
    if !warning.is_empty() {
        notes.push(warning);
    }
    notes
}

// Rows alone cannot tell a repository with no symbols from a budget that kept none of them.
fn why_nothing_ranked(result: &MapResult) -> String {
    if result.ranked == 0 {
        "nothing in this repository parsed into symbols".to_owned()
    } else if result.candidates == 0 {
        format!(
            "--max-files kept none of the {} ranked files",
            result.ranked
        )
    } else {
        format!(
            "the {}-token budget left room for none of {} symbols; raise --budget",
            result.budget, result.candidates
        )
    }
}

// An empty section prints nothing, unless the walk stopped early: then it says so.
fn companions_text(companions: &Companions) -> String {
    let note = companions.exhausted.then(|| WALK_CAPPED.to_owned());
    let rows = companions.rows.rows();
    if rows.is_empty() {
        return note.map(|line| line + "\n").unwrap_or_default();
    }
    let mut lines = vec!["tests exercising the files above:".to_owned()];
    lines.extend(rows.iter().map(companion_row));
    let omitted = companions.rows.omitted();
    if omitted > 0 {
        lines.push(omitted_line(omitted, "test files", companions.rows.limit()));
    }
    lines.extend(note);
    lines.join("\n") + "\n"
}

// The span is one referencing symbol; the coverage clause is about the whole file.
fn companion_row(row: &Companion) -> String {
    let located = format!("{}:{}-{}", one_line(&row.path), row.start, row.end);
    if row.covers.is_empty() {
        return format!("  {located}");
    }
    let covers: Vec<String> = row
        .covers
        .iter()
        .map(|path| one_line(path).into_owned())
        .collect();
    format!("  {located}  -- file references {}", covers.join(", "))
}

#[derive(Clone, Serialize)]
pub(crate) struct MapJson<'a> {
    budget_tokens: usize,
    symbols_included: usize,
    symbols_available: usize,
    files_ranked: usize,
    rendered_tokens: usize,
    rank_converged: bool,
    seeds: &'a [String],
    unresolved_seeds: &'a [String],
    files: Vec<FileJson<'a>>,
    skipped: Vec<SkippedJson<'a>>,
    test_companions: CompanionsJson<'a>,
}

#[derive(Clone, Serialize)]
struct FileJson<'a> {
    path: &'a str,
    rank: f64,
    symbols: Vec<EntryJson<'a>>,
    omitted: usize,
    reached: bool,
    commits_90d: Option<usize>,
    last_commit_days: Option<i64>,
}

#[derive(Clone, Serialize)]
struct EntryJson<'a> {
    line: u32,
    signature: &'a str,
    stable_id: &'a str,
    depth: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    rationales: Vec<RationaleJson<'a>>,
}

#[derive(Clone, Serialize)]
pub(crate) struct RationaleJson<'a> {
    stable_id: &'a str,
    parent_id: &'a str,
    line: u32,
    kind: &'a str,
    text: &'a str,
    citations: &'a [String],
}

impl<'a> RationaleJson<'a> {
    pub(crate) fn of(node: &'a RationaleNode) -> Self {
        RationaleJson {
            stable_id: &node.stable_id,
            parent_id: &node.parent_id,
            line: node.line,
            kind: node.kind,
            text: &node.text,
            citations: &node.citations,
        }
    }
}

#[derive(Clone, Serialize)]
struct CompanionsJson<'a> {
    total: usize,
    limit: usize,
    omitted: usize,
    exhausted: bool,
    rows: Vec<CompanionJson<'a>>,
}

#[derive(Clone, Serialize)]
struct CompanionJson<'a> {
    path: &'a str,
    start: u32,
    end: u32,
    covers: &'a [String],
    depth: u32,
    weight: f64,
}

impl<'a> MapJson<'a> {
    pub(crate) fn of(result: &'a MapResult) -> Self {
        MapJson {
            budget_tokens: result.budget,
            symbols_included: result.included,
            symbols_available: result.candidates,
            files_ranked: result.ranked,
            rendered_tokens: result.rendered,
            rank_converged: result.rank_converged,
            seeds: &result.seeds,
            unresolved_seeds: &result.unresolved_seeds,
            files: result.files.iter().map(FileJson::of).collect(),
            skipped: skipped_json(&result.skipped),
            test_companions: CompanionsJson::of(&result.companions),
        }
    }
}

impl<'a> FileJson<'a> {
    fn of(file: &'a MapFile) -> Self {
        FileJson {
            path: &file.path,
            rank: round_half_even(file.rank, 6),
            symbols: file.entries.iter().map(EntryJson::of).collect(),
            omitted: file.omitted(),
            reached: file.reached,
            commits_90d: file.commits_window,
            last_commit_days: file.last_commit_days,
        }
    }
}

impl<'a> EntryJson<'a> {
    fn of(entry: &'a MapEntry) -> Self {
        EntryJson {
            line: entry.line,
            signature: &entry.signature,
            stable_id: &entry.stable_id,
            depth: entry.depth,
            rationales: entry.rationales.iter().map(RationaleJson::of).collect(),
        }
    }
}

impl<'a> CompanionsJson<'a> {
    fn of(companions: &'a Companions) -> Self {
        let rows = companions.rows.rows().iter().map(|row| CompanionJson {
            path: &row.path,
            start: row.start,
            end: row.end,
            covers: &row.covers,
            depth: row.depth,
            weight: round_half_even(row.weight, 6),
        });
        CompanionsJson {
            total: companions.rows.total(),
            limit: companions.rows.limit(),
            omitted: companions.rows.omitted(),
            exhausted: companions.exhausted,
            rows: rows.collect(),
        }
    }
}

impl Listing for MapJson<'_> {
    fn items(&self) -> usize {
        self.files.len()
    }

    fn prefix(&self, kept: usize) -> Self {
        let mut prefix = self.clone();
        prefix.files.truncate(kept);
        prefix
    }
}
