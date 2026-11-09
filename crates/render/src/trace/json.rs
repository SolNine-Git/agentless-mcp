//! The JSON form of a trace answer: each card's typed values, the cards in request order, which the
//! token ceiling trims as whole cards.

use app::search::{FileRow, Measure, Place};
use app::trace::{
    Callees, Callers, Card, Change, Cycles, Declares, DiffCounts, DiffFile, DiffFiles, DiffMoved,
    DiffSection, DiffTrace, FileLinks, FileStatus, FileTrace, FolderEntry, FolderTrace, Linked,
    Miss, Outcome, RangeTrace, Section, Similar, SymbolTrace, Tally, Tests, Trace, TraceRequest,
};
use app::{EdgeRow, HistoryResult, SymbolCard};
use serde::Serialize;

use crate::envelope::Listing;
use crate::history::HistoryJson;
use crate::refs::{CoverageJson, GroupJson, SharedRowJson};
use crate::symbols::{CardJson, UsageJson};

#[derive(Clone, Serialize)]
pub(super) struct TraceJson<'a> {
    more: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rev: Option<RevisionJson<'a>>,
    cards: Vec<TracedJson<'a>>,
}

#[derive(Clone, Serialize)]
struct RevisionJson<'a> {
    name: &'a str,
    commit: &'a str,
}

impl<'a> TraceJson<'a> {
    pub(super) fn of(request: &TraceRequest, trace: &'a Trace) -> Self {
        let cards = trace.cards.iter().map(|traced| TracedJson {
            target: &traced.target,
            outcome: OutcomeJson::of(&traced.outcome),
        });
        let rev = (trace.revision.as_ref()).map(|revision| RevisionJson {
            name: &revision.name,
            commit: &revision.commit,
        });
        TraceJson {
            more: request.more.map(Section::as_str),
            rev,
            cards: cards.collect(),
        }
    }
}

// The ceiling drops whole cards, the list the request names.
impl Listing for TraceJson<'_> {
    fn items(&self) -> usize {
        self.cards.len()
    }

    fn prefix(&self, kept: usize) -> Self {
        let mut prefix = self.clone();
        prefix.cards.truncate(kept);
        prefix
    }
}

#[derive(Clone, Serialize)]
struct TracedJson<'a> {
    target: &'a str,
    #[serde(flatten)]
    outcome: OutcomeJson<'a>,
}

#[derive(Clone, Serialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
enum OutcomeJson<'a> {
    Symbol(Box<SymbolJson<'a>>),
    File(Box<FileJson<'a>>),
    Folder(FolderJson<'a>),
    Candidates {
        total: usize,
        rows: Vec<CardJson<'a>>,
    },
    Miss(MissJson<'a>),
    Range(RangeJson<'a>),
    Diff(Box<DiffJson<'a>>),
    Same {
        card: usize,
    },
    NoSection {
        section: &'static str,
        kind: &'static str,
    },
}

impl<'a> OutcomeJson<'a> {
    fn of(outcome: &'a Outcome) -> Self {
        match outcome {
            Outcome::Card(Card::Symbol(traced)) => Self::Symbol(Box::new(SymbolJson::of(traced))),
            Outcome::Card(Card::File(traced)) => Self::File(Box::new(FileJson::of(traced))),
            Outcome::Card(Card::Folder(traced)) => Self::Folder(FolderJson::of(traced)),
            Outcome::Card(Card::Candidates(traced)) => Self::Candidates {
                total: traced.total,
                rows: traced.rows.iter().map(CardJson::of).collect(),
            },
            Outcome::Card(Card::Miss(missed)) => Self::Miss(MissJson::of(missed)),
            Outcome::Card(Card::Range(traced)) => Self::Range(RangeJson::of(traced)),
            Outcome::Card(Card::Diff(traced)) => Self::Diff(Box::new(DiffJson::of(traced))),
            Outcome::Same(first) => Self::Same { card: *first },
            Outcome::NoSection { section, kind } => Self::NoSection {
                section: section.as_str(),
                kind: kind.as_str(),
            },
        }
    }
}

// A count, and whether it is complete or the floor of a list cut before the count.
#[derive(Clone, Copy, Serialize)]
struct TallyJson {
    count: usize,
    complete: bool,
}

impl TallyJson {
    fn of(tally: Tally) -> Self {
        let (count, complete) = match tally {
            Tally::Exact(count) => (count, true),
            Tally::Floor(count) => (count, false),
        };
        TallyJson { count, complete }
    }
}

// A section that could not be read gives its reason in place of its value.
#[derive(Clone, Serialize)]
#[serde(untagged)]
enum ReadJson<'a, T> {
    Read(T),
    Unread { unread: &'a str },
}

impl<'a, T> ReadJson<'a, T> {
    fn of<V>(read: &'a Result<V, String>, value: impl Fn(&'a V) -> T) -> Self {
        match read {
            Ok(read) => ReadJson::Read(value(read)),
            Err(reason) => ReadJson::Unread { unread: reason },
        }
    }
}

#[derive(Clone, Serialize)]
struct SymbolJson<'a> {
    symbol: CardJson<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    callers: Option<CallersJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    callees: Option<CalleesJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tests: Option<TestsJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    similar: Option<SimilarJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_change: Option<ChangeJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    history: Option<ReadJson<'a, HistoryJson<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<ReadJson<'a, CardJson<'a>>>,
}

impl<'a> SymbolJson<'a> {
    fn of(traced: &'a SymbolTrace) -> Self {
        let history = |result: &'a HistoryResult| HistoryJson::of(result);
        let body = |card: &'a SymbolCard| CardJson::of(card);
        SymbolJson {
            symbol: CardJson::of(&traced.card),
            callers: traced.callers.as_ref().map(CallersJson::of),
            callees: traced.callees.as_ref().map(CalleesJson::of),
            tests: traced.tests.as_ref().map(TestsJson::of),
            similar: traced.similar.as_ref().map(SimilarJson::of),
            last_change: traced.change.as_ref().map(ChangeJson::of),
            history: (traced.history.as_ref()).map(|read| ReadJson::of(read, history)),
            body: (traced.body.as_ref()).map(|read| ReadJson::of(read, body)),
        }
    }
}

#[derive(Clone, Serialize)]
struct CallersJson<'a> {
    bound: usize,
    name_only: usize,
    name_only_files: TallyJson,
    files: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    tiers: Option<Vec<TierSitesJson>>,
    rows: Vec<GroupJson<'a>>,
    rows_total: TallyJson,
    coverage: CoverageJson,
}

impl<'a> CallersJson<'a> {
    fn of(callers: &'a Callers) -> Self {
        let tiers = callers.tiers.as_ref().map(|tiers| {
            let counted = tiers.iter().map(|tier| TierSitesJson {
                tier: tier.tier,
                sites: tier.sites,
                files: tier.files,
            });
            counted.collect()
        });
        CallersJson {
            bound: callers.bound,
            name_only: callers.name_only,
            name_only_files: TallyJson::of(callers.name_only_files),
            files: callers.files,
            tiers,
            rows: callers.rows.iter().map(GroupJson::of).collect(),
            rows_total: TallyJson::of(callers.rows_total),
            coverage: CoverageJson::of(callers.coverage),
        }
    }
}

#[derive(Clone, Serialize)]
struct TierSitesJson {
    tier: &'static str,
    sites: usize,
    files: usize,
}

#[derive(Clone, Serialize)]
struct TierEdgesJson {
    tier: &'static str,
    edges: usize,
}

#[derive(Clone, Serialize)]
struct CalleesJson<'a> {
    bound: usize,
    name_only: usize,
    tiers: Vec<TierEdgesJson>,
    rows: Vec<EdgeGroupJson<'a>>,
    rows_total: TallyJson,
}

impl<'a> CalleesJson<'a> {
    fn of(callees: &'a Callees) -> Self {
        let tiers = callees.tiers.iter().map(|tier| TierEdgesJson {
            tier: tier.tier,
            edges: tier.total,
        });
        let rows = callees.rows.iter().map(|group| EdgeGroupJson {
            path: &group.path,
            tier: group.tier,
            edges: group.edges.iter().map(EdgeJson::of).collect(),
        });
        CalleesJson {
            bound: callees.bound,
            name_only: callees.name_only,
            tiers: tiers.collect(),
            rows: rows.collect(),
            rows_total: TallyJson::of(callees.rows_total),
        }
    }
}

#[derive(Clone, Serialize)]
struct EdgeGroupJson<'a> {
    path: &'a str,
    tier: &'static str,
    edges: Vec<EdgeJson<'a>>,
}

#[derive(Clone, Serialize)]
struct EdgeJson<'a> {
    node: &'a str,
    label: &'a str,
    line: u32,
}

impl<'a> EdgeJson<'a> {
    fn of(edge: &'a EdgeRow) -> Self {
        EdgeJson {
            node: &edge.node,
            label: &edge.label,
            line: edge.line,
        }
    }
}

#[derive(Clone, Serialize)]
struct TestsJson<'a> {
    rows: Vec<GroupJson<'a>>,
    total: TallyJson,
}

impl<'a> TestsJson<'a> {
    fn of(tests: &'a Tests) -> Self {
        TestsJson {
            rows: tests.rows.iter().map(GroupJson::of).collect(),
            total: TallyJson::of(tests.total),
        }
    }
}

#[derive(Clone, Serialize)]
struct SimilarJson<'a> {
    total: usize,
    rows: Vec<SimilarRowJson<'a>>,
    bound_total: TallyJson,
}

#[derive(Clone, Serialize)]
struct SimilarRowJson<'a> {
    #[serde(flatten)]
    candidate: SharedRowJson<'a>,
    tier: &'static str,
}

impl<'a> SimilarJson<'a> {
    fn of(similar: &'a Similar) -> Self {
        let rows = similar.rows.iter().map(|row| SimilarRowJson {
            candidate: SharedRowJson::of(&row.candidate),
            tier: row.tier,
        });
        SimilarJson {
            total: similar.total,
            rows: rows.collect(),
            bound_total: TallyJson::of(similar.bound_total),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum ChangeJson<'a> {
    Commit {
        sha: &'a str,
        authored: &'a str,
        subject: &'a str,
    },
    NotRead {
        seconds: u64,
    },
    // An object like every other outcome, so a reader keys on the outcome's name alone.
    NoCommit {},
    NotRepository {
        note: &'a str,
    },
    Failed {
        note: &'a str,
    },
    Unavailable {
        reason: &'a str,
    },
}

impl<'a> ChangeJson<'a> {
    fn of(change: &'a Change) -> Self {
        match change {
            Change::Commit(entry) => ChangeJson::Commit {
                sha: &entry.sha,
                authored: &entry.authored,
                subject: &entry.subject,
            },
            Change::NotRead { seconds } => ChangeJson::NotRead { seconds: *seconds },
            Change::NoCommit => ChangeJson::NoCommit {},
            Change::NotRepository { note } => ChangeJson::NotRepository { note },
            Change::Failed(note) => ChangeJson::Failed { note },
            Change::Unavailable(reason) => ChangeJson::Unavailable { reason },
        }
    }
}

#[derive(Clone, Serialize)]
struct FileJson<'a> {
    path: &'a str,
    language: &'static str,
    lines: u32,
    id_prefix: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    no_symbol_at: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    declares: Option<DeclaresJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    imports: Option<LinkedJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    importers: Option<LinkedJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tests: Option<LinkedJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cycles: Option<CyclesJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_change: Option<ChangeJson<'a>>,
}

impl<'a> FileJson<'a> {
    fn of(file: &'a FileTrace) -> Self {
        FileJson {
            path: &file.path,
            language: file.language,
            lines: file.lines,
            id_prefix: file.id_prefix,
            no_symbol_at: file.no_symbol_at,
            declares: file.declares.as_ref().map(DeclaresJson::of),
            imports: file.imports.as_ref().map(LinkedJson::of),
            importers: file.importers.as_ref().map(LinkedJson::of),
            tests: file.tests.as_ref().map(LinkedJson::of),
            cycles: file.cycles.as_ref().map(CyclesJson::of),
            last_change: file.change.as_ref().map(ChangeJson::of),
        }
    }
}

#[derive(Clone, Serialize)]
struct DeclaresJson<'a> {
    total: usize,
    kinds: Vec<KindJson>,
    rows: Vec<CardJson<'a>>,
}

#[derive(Clone, Serialize)]
struct KindJson {
    kind: &'static str,
    count: usize,
}

impl<'a> DeclaresJson<'a> {
    fn of(declares: &'a Declares) -> Self {
        let kinds = (declares.kinds.iter()).map(|&(kind, count)| KindJson { kind, count });
        DeclaresJson {
            total: declares.total,
            kinds: kinds.collect(),
            rows: declares.rows.iter().map(CardJson::of).collect(),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(untagged)]
enum LinkedJson<'a> {
    Found {
        rows: Vec<PlaceJson<'a>>,
        total: TallyJson,
    },
    NoAnchor {
        no_anchor: bool,
    },
}

impl<'a> LinkedJson<'a> {
    fn of(linked: &'a Linked<FileLinks>) -> Self {
        match linked {
            Linked::Found(links) => LinkedJson::Found {
                rows: places(&links.rows),
                total: TallyJson::of(links.total),
            },
            Linked::NoAnchor => LinkedJson::NoAnchor { no_anchor: true },
        }
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct PlaceJson<'a> {
    path: &'a str,
    line: u32,
}

// Each place of a list, in its order.
pub(crate) fn places(places: &[Place]) -> Vec<PlaceJson<'_>> {
    let each = places.iter().map(|place| PlaceJson {
        path: &place.path,
        line: place.line,
    });
    each.collect()
}

#[derive(Clone, Serialize)]
struct CyclesJson<'a> {
    rows: Vec<CycleJson<'a>>,
    total: TallyJson,
}

#[derive(Clone, Serialize)]
struct CycleJson<'a> {
    files: &'a [String],
    members: &'a [String],
}

impl<'a> CyclesJson<'a> {
    fn of(cycles: &'a Cycles) -> Self {
        let rows = cycles.rows.iter().map(|cycle| CycleJson {
            files: &cycle.files,
            members: &cycle.members,
        });
        CyclesJson {
            rows: rows.collect(),
            total: TallyJson::of(cycles.total),
        }
    }
}

#[derive(Clone, Serialize)]
struct FolderJson<'a> {
    path: &'a str,
    files: usize,
    lines: u64,
    total: usize,
    depth: usize,
    entries: Vec<EntryJson<'a>>,
}

impl<'a> FolderJson<'a> {
    fn of(folder: &'a FolderTrace) -> Self {
        FolderJson {
            path: &folder.path,
            files: folder.files,
            lines: folder.lines,
            total: folder.total,
            depth: folder.depth,
            entries: folder.entries.iter().map(EntryJson::of).collect(),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
enum EntryJson<'a> {
    File {
        depth: usize,
        #[serde(flatten)]
        row: RowJson<'a>,
    },
    Folder {
        depth: usize,
        path: &'a str,
        files: usize,
    },
}

impl<'a> EntryJson<'a> {
    fn of(entry: &'a FolderEntry) -> Self {
        match entry {
            FolderEntry::File { depth, row } => EntryJson::File {
                depth: *depth,
                row: RowJson::of(row),
            },
            FolderEntry::Folder { depth, path, files } => EntryJson::Folder {
                depth: *depth,
                path,
                files: *files,
            },
        }
    }
}

// One file's row as a folder card and a file listing publish it.
#[derive(Clone, Serialize)]
pub(crate) struct RowJson<'a> {
    path: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    lines: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    symbols: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unread: Option<&'a str>,
}

impl<'a> RowJson<'a> {
    pub(crate) fn of(row: &'a FileRow) -> Self {
        let (lines, symbols, unread) = match &row.measure {
            Measure::Counted { lines, symbols } => (Some(*lines), *symbols, None),
            Measure::Unread(reason) => (None, None, Some(reason.as_str())),
        };
        RowJson {
            path: &row.path,
            lines,
            symbols,
            unread,
        }
    }
}

#[derive(Clone, Serialize)]
struct RangeJson<'a> {
    path: &'a str,
    lines: u32,
    start: u32,
    end: u32,
    text: Vec<NumberedJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next: Option<SpanJson>,
    symbols: Vec<CardJson<'a>>,
    symbols_total: usize,
}

#[derive(Clone, Serialize)]
struct NumberedJson<'a> {
    line: u32,
    text: &'a str,
}

#[derive(Clone, Copy, Serialize)]
struct SpanJson {
    start: u32,
    end: u32,
}

impl<'a> RangeJson<'a> {
    fn of(range: &'a RangeTrace) -> Self {
        let text =
            (range.text.iter().zip(range.start..)).map(|(text, line)| NumberedJson { line, text });
        RangeJson {
            path: &range.path,
            lines: range.lines,
            start: range.start,
            end: range.end,
            text: text.collect(),
            next: range.next().map(|(start, end)| SpanJson { start, end }),
            symbols: range.symbols.iter().map(CardJson::of).collect(),
            symbols_total: range.symbols_total,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(tag = "miss", rename_all = "snake_case")]
enum MissJson<'a> {
    Nothing {
        name: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<UsageJson<'a>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        literal: Option<LiteralJson<'a>>,
    },
    AbsentAtRevision {
        path: &'a str,
    },
    UnknownPath {
        path: &'a str,
        endings: &'a [String],
    },
    NotParsed {
        path: &'a str,
        reason: &'a str,
    },
    NoGrammar {
        path: &'a str,
    },
    NotScanned {
        path: &'a str,
    },
    LinePastEnd {
        path: &'a str,
        line: u32,
        lines: u32,
    },
    RangePastEnd {
        path: &'a str,
        start: u32,
        end: u32,
        lines: u32,
    },
    BadRange {
        path: &'a str,
        start: u32,
        end: u32,
    },
    Binary {
        path: &'a str,
    },
}

#[derive(Clone, Serialize)]
struct LiteralJson<'a> {
    places: Vec<PlaceJson<'a>>,
    files: usize,
}

impl<'a> MissJson<'a> {
    fn of(miss: &'a Miss) -> Self {
        match miss {
            Miss::Nothing {
                name,
                usage,
                literal,
            } => MissJson::Nothing {
                name,
                usage: usage.as_ref().map(UsageJson::of),
                literal: literal.as_ref().map(|(places, files)| LiteralJson {
                    places: self::places(places),
                    files: *files,
                }),
            },
            Miss::AbsentAtRevision { path } => MissJson::AbsentAtRevision { path },
            Miss::UnknownPath { path, endings } => MissJson::UnknownPath { path, endings },
            Miss::NotParsed { path, reason } => MissJson::NotParsed { path, reason },
            Miss::NoGrammar { path } => MissJson::NoGrammar { path },
            Miss::NotScanned { path } => MissJson::NotScanned { path },
            Miss::LinePastEnd { .. }
            | Miss::RangePastEnd { .. }
            | Miss::BadRange { .. }
            | Miss::Binary { .. } => MissJson::of_lines(miss),
        }
    }

    // A miss of a file's lines, as `lines_miss` words it in the text.
    fn of_lines(miss: &'a Miss) -> Self {
        match miss {
            Miss::LinePastEnd { path, line, lines } => MissJson::LinePastEnd {
                path,
                line: *line,
                lines: *lines,
            },
            Miss::RangePastEnd {
                path,
                start,
                end,
                lines,
            } => MissJson::RangePastEnd {
                path,
                start: *start,
                end: *end,
                lines: *lines,
            },
            Miss::BadRange { path, start, end } => MissJson::BadRange {
                path,
                start: *start,
                end: *end,
            },
            Miss::Binary { path } => MissJson::Binary { path },
            other => unreachable!("{other:?} is no miss of a file's lines"),
        }
    }
}

// A diff card: the merge base, the files and folders, and each symbol section's rows.
#[derive(Clone, Serialize)]
struct DiffJson<'a> {
    rev: &'a str,
    scope: Option<&'a str>,
    base: &'a str,
    commits: usize,
    full: bool,
    files: DiffFilesJson<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    symbols: Option<DiffSymbolsJson<'a>>,
}

impl<'a> DiffJson<'a> {
    fn of(card: &'a DiffTrace) -> Self {
        let symbols = card.symbols.as_ref().map(|symbols| DiffSymbolsJson {
            changed: DiffSectionJson::of(&symbols.changed),
            added: DiffSectionJson::of(&symbols.added),
            removed: DiffSectionJson::of(&symbols.removed),
            moved: DiffMovedJson::of(&symbols.moved),
        });
        DiffJson {
            rev: &card.rev,
            scope: card.scope.as_deref(),
            base: &card.base,
            commits: card.commits,
            full: card.full,
            files: DiffFilesJson::of(&card.files),
            symbols,
        }
    }
}

#[derive(Clone, Serialize)]
struct DiffFilesJson<'a> {
    total: usize,
    added: usize,
    removed: usize,
    changed: usize,
    renamed: usize,
    folders_total: usize,
    folders: Vec<DiffFolderJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rows: Option<Vec<DiffFileJson<'a>>>,
}

impl<'a> DiffFilesJson<'a> {
    fn of(files: &'a DiffFiles) -> Self {
        let folders = files.folders.iter().map(|folder| DiffFolderJson {
            path: &folder.path,
            files: folder.files,
            symbols: CountsJson::of(folder.symbols),
        });
        let rows = (files.rows.as_ref()).map(|rows| rows.iter().map(DiffFileJson::of).collect());
        DiffFilesJson {
            total: files.total,
            added: files.added,
            removed: files.removed,
            changed: files.changed,
            renamed: files.renamed,
            folders_total: files.folders_total,
            folders: folders.collect(),
            rows,
        }
    }
}

#[derive(Clone, Serialize)]
struct DiffFolderJson<'a> {
    path: &'a str,
    files: usize,
    #[serde(flatten)]
    symbols: CountsJson,
}

#[derive(Clone, Serialize)]
struct DiffFileJson<'a> {
    path: &'a str,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    renamed_from: Option<&'a str>,
    #[serde(flatten)]
    symbols: CountsJson,
}

#[derive(Clone, Copy, Serialize)]
struct CountsJson {
    added: usize,
    removed: usize,
    changed: usize,
}

impl CountsJson {
    fn of(counts: DiffCounts) -> Self {
        CountsJson {
            added: counts.added,
            removed: counts.removed,
            changed: counts.changed,
        }
    }
}

impl<'a> DiffFileJson<'a> {
    fn of(file: &'a DiffFile) -> Self {
        let (status, renamed_from) = match &file.status {
            FileStatus::Added => ("added", None),
            FileStatus::Removed => ("removed", None),
            FileStatus::Changed => ("changed", None),
            FileStatus::Renamed(from) => ("renamed", Some(from.as_str())),
        };
        DiffFileJson {
            path: &file.path,
            status,
            renamed_from,
            symbols: CountsJson::of(file.symbols),
        }
    }
}

#[derive(Clone, Serialize)]
struct DiffSymbolsJson<'a> {
    changed: DiffSectionJson<'a>,
    added: DiffSectionJson<'a>,
    removed: DiffSectionJson<'a>,
    moved: DiffMovedJson<'a>,
}

#[derive(Clone, Serialize)]
struct DiffSectionJson<'a> {
    total: usize,
    files: usize,
    tests: usize,
    rows: Vec<DiffRowJson<'a>>,
}

impl<'a> DiffSectionJson<'a> {
    fn of(section: &'a DiffSection) -> Self {
        let rows = section.rows.iter().map(|row| DiffRowJson {
            card: CardJson::of(&row.card),
            base_start_line: row.base.map(|(start, _)| start),
            base_end_line: row.base.map(|(_, end)| end),
        });
        DiffSectionJson {
            total: section.total,
            files: section.files,
            tests: section.tests,
            rows: rows.collect(),
        }
    }
}

#[derive(Clone, Serialize)]
struct DiffRowJson<'a> {
    #[serde(flatten)]
    card: CardJson<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_end_line: Option<u32>,
}

#[derive(Clone, Serialize)]
struct DiffMovedJson<'a> {
    total: usize,
    rows: Vec<MovedJson<'a>>,
}

impl<'a> DiffMovedJson<'a> {
    fn of(moved: &'a DiffMoved) -> Self {
        let rows = moved.rows.iter().map(|row| MovedJson {
            from: CardJson::of(&row.from),
            to: CardJson::of(&row.to),
        });
        DiffMovedJson {
            total: moved.total,
            rows: rows.collect(),
        }
    }
}

#[derive(Clone, Serialize)]
struct MovedJson<'a> {
    from: CardJson<'a>,
    to: CardJson<'a>,
}
