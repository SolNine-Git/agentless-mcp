//! The trace of code targets: one card per target that says what the target is and how it
//! connects, each section cut to its default row cap with its complete count, or one section in
//! full. The cards are typed values; the renderer words them.

mod change;
mod file;
mod resolve;
mod section;
mod symbol;

use std::time::Duration;

use git::RepoFile;
use graph::NameIndex;

use crate::bounds::within;
use crate::ops::limits::{FIND_USAGE_FILES, MAX_LIMIT};
use crate::scan::over_scan;
use crate::search::{FileRow, Place, walked_files};
use crate::symbol_graph::{CallGraph, with_call_graph};
use crate::{
    CycleRow, EdgeRow, Error, HistoryEntry, HistoryResult, IntArg, Noted, Reading, RefGroup,
    Services, SharedCaller, SymbolCard, Usage,
};
use resolve::Resolved;

pub use section::{CardKind, Section};

/// The most targets that one trace call takes.
pub const MAX_TARGETS: usize = 8;
/// The rows that a symbol card shows of each of its callers, tests, callees and similar sections.
pub const SYMBOL_ROWS: usize = 3;
/// The declarations that a file card shows.
pub const FILE_DECLARATIONS: usize = 8;
/// The rows that a file card shows of each of its importers, tests and imports sections.
pub const FILE_ROWS: usize = 3;
/// The import cycles that a file card shows.
pub const FILE_CYCLES: usize = 1;
/// The definitions that a candidates card shows.
pub const CANDIDATE_ROWS: usize = 16;
/// The usage files that a miss shows.
pub const MISS_USAGE_FILES: usize = FIND_USAGE_FILES;
/// The files that the literal scan of a miss shows.
pub const MISS_LITERAL_FILES: usize = 3;
/// The folder levels that a folder card shows.
pub const FOLDER_DEPTH: usize = 1;
/// The entries that a folder card shows.
pub const FOLDER_ENTRIES: usize = 60;
/// The folder levels that `more=tree` shows.
pub const MORE_TREE_DEPTH: usize = 4;
/// The entries that `more=tree` shows.
pub const MORE_TREE_ENTRIES: usize = 500;
/// The most sites, edges or candidates that a symbol card's section lists with `more`.
pub const LISTING_LIMIT: usize = MAX_LIMIT;
/// The time that a symbol card or a file card waits for git to read its last change.
pub const LAST_CHANGE_DEADLINE: Duration = Duration::from_secs(2);

/// A trace request: the targets, in the caller's order, and the one section to show in full.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceRequest {
    /// Each target: a stable id, a `path:line`, a repository-relative path, or a name.
    pub targets: Vec<String>,
    /// The section that each card shows in full, alone, or `None` for the default card.
    pub more: Option<Section>,
}

/// One answer per target, in the order of the request.
#[derive(Clone, Debug, PartialEq)]
pub struct Trace {
    /// The answers.
    pub cards: Vec<Traced>,
}

/// The answer for one target.
#[derive(Clone, Debug, PartialEq)]
pub struct Traced {
    /// The target as the caller wrote it.
    pub target: String,
    /// The card, or why the answer has none.
    pub outcome: Outcome,
}

/// What a target's answer holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// The target's card.
    Card(Card),
    /// The target repeats an earlier one, at this 0-based position, whose card it shares.
    Same(usize),
    /// `more` names a section that this kind of card does not hold.
    NoSection {
        /// The section asked for.
        section: Section,
        /// The kind of the target's card.
        kind: CardKind,
    },
}

/// One card.
#[derive(Clone, Debug, PartialEq)]
pub enum Card {
    /// A symbol: its signature and how it connects.
    Symbol(Box<SymbolTrace>),
    /// A file: its declarations, imports, importers, tests and import cycles.
    File(Box<FileTrace>),
    /// A folder: its entries.
    Folder(FolderTrace),
    /// A name with several definitions.
    Candidates(CandidatesTrace),
    /// A target that matched nothing, and where the next call can look.
    Miss(Miss),
}

impl Card {
    /// Returns the kind of the card.
    #[must_use]
    pub fn kind(&self) -> CardKind {
        match self {
            Card::Symbol(_) => CardKind::Symbol,
            Card::File(_) => CardKind::File,
            Card::Folder(_) => CardKind::Folder,
            Card::Candidates(_) => CardKind::Candidates,
            Card::Miss(_) => CardKind::Miss,
        }
    }
}

/// A count that is exact, or a floor when the list it counts was cut before the count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tally {
    /// The complete count.
    Exact(usize),
    /// The count of the listed part: the true count is this or more.
    Floor(usize),
}

impl Tally {
    // `whole` says whether the counted list held every item.
    fn of(count: usize, whole: bool) -> Tally {
        if whole {
            Tally::Exact(count)
        } else {
            Tally::Floor(count)
        }
    }
}

/// A symbol's card. In a default card every section but `history` and `body` is present; with
/// `more`, only that section is.
#[derive(Clone, Debug, PartialEq)]
pub struct SymbolTrace {
    /// The definition: stable id, path, span, kind and signature.
    pub card: SymbolCard,
    /// The sites that reference the symbol.
    pub callers: Option<Callers>,
    /// The symbols that the symbol references.
    pub callees: Option<Callees>,
    /// The test files that reference the symbol.
    pub tests: Option<Tests>,
    /// The symbols that the symbol's callers also reference.
    pub similar: Option<Similar>,
    /// The newest commit that touched the symbol's lines.
    pub change: Option<Change>,
    /// The commits that touched the symbol's lines, or the history refusal's text.
    pub history: Option<Result<HistoryResult, String>>,
    /// The card with its numbered body, or why the body could not be read.
    pub body: Option<Result<SymbolCard, String>>,
}

/// The sites that reference a symbol, by file and evidence tier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Callers {
    /// The sites at a binding tier, same-file or resolved-via-import.
    pub bound: usize,
    /// The sites at a name-only tier, unique or ambiguous.
    pub name_only: usize,
    /// The files that hold a name-only site.
    pub name_only_files: Tally,
    /// The files that hold any site.
    pub files: usize,
    /// The sites and files of each tier, strongest first, or `None` when the listing was cut.
    pub tiers: Option<Vec<TierSites>>,
    /// The groups that the card lists. By default: the binding-tier groups outside test paths, in
    /// path order, cut at the cap. With `more`: every group that the listing kept, binding tiers
    /// first, then the other tiers in tier order.
    pub rows: Vec<RefGroup>,
    /// The groups that `rows` lists before the cap.
    pub rows_total: Tally,
    /// How much of the repository the fan-in checked.
    pub coverage: crate::Coverage,
}

/// The sites and files of one evidence tier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TierSites {
    /// The tier's wire value, such as `imported`.
    pub tier: &'static str,
    /// The sites.
    pub sites: usize,
    /// The files.
    pub files: usize,
}

/// The symbols that a symbol references, by evidence tier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Callees {
    /// The edges at a binding tier, same-file or resolved-via-import.
    pub bound: usize,
    /// The edges at a name-only tier, unique or ambiguous.
    pub name_only: usize,
    /// The edges of each tier, strongest first.
    pub tiers: Vec<TierTotal>,
    /// The edges grouped by file and tier, in edge order. By default: the binding tiers, cut at
    /// the cap. With `more`: every tier, strongest first.
    pub rows: Vec<EdgeGroup>,
    /// The groups that `rows` lists before the cap.
    pub rows_total: Tally,
}

/// The edge count of one evidence tier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TierTotal {
    /// The tier's wire value.
    pub tier: &'static str,
    /// The edges.
    pub total: usize,
}

/// The edges of one file at one tier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeGroup {
    /// The file of the other ends.
    pub path: String,
    /// The tier's wire value.
    pub tier: &'static str,
    /// The edges, in edge order.
    pub edges: Vec<EdgeRow>,
}

/// The test files that reference a symbol at a tier better than name-only-ambiguous.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tests {
    /// The groups, in path order, cut at the cap.
    pub rows: Vec<RefGroup>,
    /// The groups.
    pub total: Tally,
}

/// The symbols that a symbol's callers also reference.
#[derive(Clone, Debug, PartialEq)]
pub struct Similar {
    /// Every candidate that two or more callers share.
    pub total: usize,
    /// The candidates that the card lists. By default: those that share a binding-tier caller,
    /// best first, cut at the cap. With `more`: every candidate whose tier the listing shows,
    /// binding tiers first, then the other tiers in tier order, each best first.
    pub rows: Vec<SimilarRow>,
    /// The candidates that share a binding-tier caller. Zero means the ranking runs only through
    /// name-only callers.
    pub bound_total: Tally,
}

/// A candidate of the similar section, and the strongest tier at which one of the callers that
/// it shares references the target.
#[derive(Clone, Debug, PartialEq)]
pub struct SimilarRow {
    /// The candidate.
    pub candidate: SharedCaller,
    /// The tier's wire value.
    pub tier: &'static str,
}

/// The newest commit on a symbol's lines or on a file, or why the card has none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// The commit.
    Commit(HistoryEntry),
    /// Git did not answer within the deadline, in whole seconds.
    NotRead {
        /// The deadline.
        seconds: u64,
    },
    /// No commit in HEAD's history touched the file: HEAD does not hold it, or has no commit.
    NoCommit,
    /// No git repository holds the root.
    NotRepository {
        /// The snapshot's note on what git could not read.
        note: String,
    },
    /// Git failed another way. The note says why.
    Failed(String),
    /// The history refusal's text, such as no git or no commit.
    Unavailable(String),
}

/// A file's card. In a default card every section is present; with `more`, only that section is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileTrace {
    /// The path below the repository root.
    pub path: String,
    /// The language's name.
    pub language: &'static str,
    /// The file's lines.
    pub lines: u32,
    /// The prefix of the file's stable ids, such as `rs`.
    pub id_prefix: &'static str,
    /// A `path:line` target that no symbol spans gives this line.
    pub no_symbol_at: Option<u32>,
    /// The declarations.
    pub declares: Option<Declares>,
    /// The files that the file imports.
    pub imports: Option<Linked<FileLinks>>,
    /// The files outside test paths that import the file.
    pub importers: Option<Linked<FileLinks>>,
    /// The test files that import the file.
    pub tests: Option<Linked<FileLinks>>,
    /// The import cycles that the file is in.
    pub cycles: Option<Cycles>,
    /// The newest commit that touched the file. Only a default card reads it.
    pub change: Option<Change>,
}

/// A file's declarations: the count of each kind and the rows the cap kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declares {
    /// Every declaration.
    pub total: usize,
    /// The count of each kind, most first.
    pub kinds: Vec<(&'static str, usize)>,
    /// The kept declarations in line order: kinds other than constants first take the cap.
    pub rows: Vec<SymbolCard>,
}

/// An import section, or why the file has none to read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Linked<T> {
    /// The section.
    Found(T),
    /// No symbol of the file joins the name graph, so its imports were not read.
    NoAnchor,
}

/// The files at the other end of a file's imports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLinks {
    /// Each file with its first import line, in edge order, cut at the cap.
    pub rows: Vec<Place>,
    /// The files.
    pub total: Tally,
}

/// The import cycles that hold a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cycles {
    /// The cycles that hold the file, cut at the cap.
    pub rows: Vec<CycleRow>,
    /// The cycles that hold the file.
    pub total: Tally,
}

/// A folder's card: its entries to a depth, cut at the entry cap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderTrace {
    /// The path below the repository root, or empty for the root.
    pub path: String,
    /// The files below the folder.
    pub files: usize,
    /// The lines of those files that could be read.
    pub lines: u64,
    /// The entries, in tree order, cut at the entry cap.
    pub entries: Vec<FolderEntry>,
    /// The entries to the depth.
    pub total: usize,
    /// The depth.
    pub depth: usize,
}

/// One entry of a folder card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderEntry {
    /// A file, with its row.
    File {
        /// The levels below the folder, from 1.
        depth: usize,
        /// The row, whose path is below the repository root.
        row: FileRow,
    },
    /// A folder, with the files below it.
    Folder {
        /// The levels below the folder, from 1.
        depth: usize,
        /// The path below the repository root.
        path: String,
        /// The files below it.
        files: usize,
    },
}

/// A name with several definitions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidatesTrace {
    /// The definitions.
    pub total: usize,
    /// The definitions in the order explain ranks them, cut at the cap.
    pub rows: Vec<SymbolCard>,
}

/// A target that matched nothing, and where its next call can look.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Miss {
    /// No symbol, file or folder matches.
    Nothing {
        /// The name whose usage files were looked for.
        name: String,
        /// The code files that spell the name and do not declare it.
        usage: Option<Usage>,
        /// With no usage file, where the target's text occurs: the first places and the files.
        literal: Option<(Vec<Place>, usize)>,
    },
    /// A path that names no file or folder, and the paths that end the same way.
    UnknownPath {
        /// The path as resolved below the root.
        path: String,
        /// Up to [`MISS_LITERAL_FILES`] paths that end with it.
        endings: Vec<String>,
    },
    /// A file that the scan passed over, and why.
    NotParsed {
        /// The path below the repository root.
        path: String,
        /// The scan's reason.
        reason: String,
    },
    /// A file that no grammar reads.
    NoGrammar {
        /// The path below the repository root.
        path: String,
    },
    /// A file that a grammar reads and the scan never reads, such as the project config file.
    NotScanned {
        /// The path below the repository root.
        path: String,
    },
    /// A `path:line` past the file's last line.
    LinePastEnd {
        /// The path below the repository root.
        path: String,
        /// The line asked for.
        line: u32,
        /// The file's lines.
        lines: u32,
    },
}

/// Traces each target of `request` in the repository that `reading` names. The warnings of the
/// walk, the scans and the cache come with either outcome.
#[must_use]
pub fn trace(
    services: &Services,
    reading: &Reading<'_>,
    request: &TraceRequest,
) -> Noted<Result<Trace, Error>> {
    let count = IntArg::from_usize(request.targets.len());
    if let Err(error) = within(&count, 1, MAX_TARGETS, "target") {
        return Noted::new(Err(error));
    }
    let walked = walked_files(services, &reading.repo.root);
    let mut diagnostics = walked.diagnostics;
    let value = walked.value.and_then(|files| {
        let scanned = over_scan(services, reading, |facts| {
            let index = NameIndex::build(&facts.files);
            let resolved = resolve::resolved(reading, facts, &index, &files, &request.targets);
            Ok(with_call_graph(facts, &index, reading.deadline, |graph| {
                let call = Call {
                    services,
                    reading,
                    files: &files,
                    graph,
                };
                carded(&call, request, resolved)
            }))
        });
        diagnostics.extend(scanned.diagnostics);
        let traced = scanned.value?;
        diagnostics.extend(traced.diagnostics);
        traced.value.map(|cards| Trace { cards })
    });
    Noted { value, diagnostics }
}

// What every card of one call reads: the walk, and the scan's one name index and graph.
struct Call<'c, 'r, 'g> {
    services: &'c Services,
    reading: &'c Reading<'r>,
    files: &'c [RepoFile],
    graph: &'c CallGraph<'g>,
}

// Each target's outcome in request order: a repeat shares the card of its first copy.
fn carded(
    call: &Call<'_, '_, '_>,
    request: &TraceRequest,
    resolved: Vec<Resolved>,
) -> Noted<Result<Vec<Traced>, Error>> {
    let mut noted = Noted::new(Vec::new());
    for (at, (target, resolved)) in request.targets.iter().zip(resolved).enumerate() {
        let first = request.targets[..at].iter().position(|seen| seen == target);
        let outcome = match first {
            Some(first) => Noted::new(Ok(Outcome::Same(first))),
            None => outcome(call, resolved, request.more),
        };
        noted.diagnostics.extend(outcome.diagnostics);
        match outcome.value {
            Ok(outcome) => noted.value.push(Traced {
                target: target.clone(),
                outcome,
            }),
            Err(error) => return noted.map(|_| Err(error)),
        }
    }
    noted.map(Ok)
}

// A candidates card and a miss show themselves whatever `more` names; a symbol, file or folder
// card that does not hold the section says so.
fn outcome(
    call: &Call<'_, '_, '_>,
    resolved: Resolved,
    more: Option<Section>,
) -> Noted<Result<Outcome, Error>> {
    let Call {
        services,
        reading,
        files,
        graph,
    } = *call;
    let kind = resolved.kind();
    let absent = more.filter(|section| !kind.sections().contains(section));
    if let Some(section) = absent
        && matches!(kind, CardKind::Symbol | CardKind::File | CardKind::Folder)
    {
        return Noted::new(Ok(Outcome::NoSection { section, kind }));
    }
    let card = match resolved {
        Resolved::Symbol(card) => symbol::symbol_trace(services, reading, graph, card, more)
            .map(|traced| traced.map(|traced| Card::Symbol(Box::new(traced)))),
        Resolved::File(found) => file::file_trace(services, reading, graph, &found, more)
            .map(|traced| traced.map(|traced| Card::File(Box::new(traced)))),
        Resolved::Folder(path) => {
            file::folder_trace(reading, files, &path, more).map(|traced| Ok(Card::Folder(traced)))
        }
        Resolved::Candidates(rows) => Noted::new(Ok(Card::Candidates(candidates(rows, more)))),
        Resolved::Missed(missed) => resolve::missed(services, reading, missed).map_ok(Card::Miss),
    };
    card.map_ok(Outcome::Card)
}

fn candidates(rows: Vec<SymbolCard>, more: Option<Section>) -> CandidatesTrace {
    let cap = cap(more, Section::Candidates, CANDIDATE_ROWS);
    let total = rows.len();
    let mut rows = rows;
    rows.truncate(cap);
    CandidatesTrace { total, rows }
}

// The rows a section keeps: its default cap, or every row when `more` names it.
fn cap(more: Option<Section>, section: Section, default: usize) -> usize {
    if more == Some(section) {
        usize::MAX
    } else {
        default
    }
}

#[cfg(test)]
mod tests;
