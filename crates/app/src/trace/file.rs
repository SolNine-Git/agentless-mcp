//! A file's card, from its scanned declarations, the imports that explain reads through one of its
//! symbols, and the import cycles; and a folder's card, from the walk and each file's row.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use facts::line_count;
use git::RepoFile;

use super::change::file_change;
use super::resolve::FoundFile;
use super::{
    Call, Cycles, Declares, FILE_CYCLES, FILE_DECLARATIONS, FILE_ROWS, FOLDER_DEPTH,
    FOLDER_ENTRIES, FileLinks, FileTrace, FolderEntry, FolderTrace, LAST_CHANGE_DEADLINE, Linked,
    MORE_TREE_DEPTH, MORE_TREE_ENTRIES, Section, Tally, cap,
};
use crate::explain::explain_in;
use crate::ops::limits::MAX_LIMIT;
use crate::search::{Place, file_row};
use crate::{
    CyclesRequest, Error, ExplainRequest, ImportRow, IntArg, Noted, Reading, Services, SymbolCard,
    cycles, is_test_path,
};
use kernel::Bounded;

// The kind whose declarations give way first when the cap cuts a file's declarations.
const CONSTANT: &str = "constant";

pub(super) fn file_trace(
    call: &Call<'_, '_, '_>,
    found: &FoundFile,
    more: Option<Section>,
) -> Noted<Result<FileTrace, Error>> {
    let (services, reading) = (call.services, call.reading);
    let bare = read_lines(&reading.repo.root, &found.path).map(|lines| bare(found, lines, more));
    let linked = [Section::Imports, Section::Importers, Section::Tests];
    let traced = Noted::new(bare).then_ok(|traced| {
        if linked.into_iter().any(|section| wanted(more, section)) {
            links(call, found, more).map_ok(|links| links.into_card(traced, more))
        } else {
            Noted::new(Ok(traced))
        }
    });
    let traced = traced.then_ok(|traced| with_cycles(services, reading, traced, more));
    traced.map_ok(|traced| {
        let change = (more.is_none())
            .then(|| file_change(services, reading, &traced.path, LAST_CHANGE_DEADLINE));
        FileTrace { change, ..traced }
    })
}

// Whether a file card computes `section`: every section by default, or the one `more` names.
fn wanted(more: Option<Section>, section: Section) -> bool {
    more.is_none_or(|asked| asked == section)
}

// The card with its header and declarations, before the operations that fill the rest.
fn bare(found: &FoundFile, lines: u32, more: Option<Section>) -> FileTrace {
    let cap = cap(more, Section::Declares, FILE_DECLARATIONS);
    FileTrace {
        path: found.path.clone(),
        language: found.language.name(),
        lines,
        id_prefix: found.language.extraction().prefix,
        no_symbol_at: found.no_symbol_at,
        declares: wanted(more, Section::Declares).then(|| declares(&found.symbols, cap)),
        imports: None,
        importers: None,
        tests: None,
        cycles: None,
        change: None,
    }
}

fn with_cycles(
    services: &Services,
    reading: &Reading<'_>,
    traced: FileTrace,
    more: Option<Section>,
) -> Noted<Result<FileTrace, Error>> {
    if !wanted(more, Section::Cycles) {
        return Noted::new(Ok(traced));
    }
    let request = CyclesRequest {
        limit: IntArg::from_usize(MAX_LIMIT),
    };
    cycles(services, reading, &request).map_ok(|report| FileTrace {
        cycles: Some(holding(&report.cycles, &traced.path, more)),
        ..traced
    })
}

fn read_lines(root: &Path, path: &str) -> Result<u32, Error> {
    match io::read_bounded(&root.join(path)) {
        Ok(bytes) => Ok(line_count(&String::from_utf8_lossy(&bytes))),
        Err(skipped) => Err(kernel::Error::OperationFailed(format!("{path}: {skipped}")).into()),
    }
}

// Every count by kind, most first; the cap keeps kinds other than constants first, and the kept
// rows read in line order.
fn declares(symbols: &[SymbolCard], cap: usize) -> Declares {
    let mut kinds: BTreeMap<&'static str, usize> = BTreeMap::new();
    for symbol in symbols {
        *kinds.entry(symbol.kind).or_default() += 1;
    }
    let mut kinds: Vec<(&'static str, usize)> = kinds.into_iter().collect();
    kinds.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let mut ranked: Vec<&SymbolCard> = symbols.iter().collect();
    ranked.sort_by_key(|symbol| (symbol.kind == CONSTANT, symbol.start_line));
    ranked.truncate(cap);
    ranked.sort_by_key(|symbol| symbol.start_line);
    Declares {
        total: symbols.len(),
        kinds,
        rows: ranked.into_iter().cloned().collect(),
    }
}

// The import sides of the file, read through its anchor symbol, or none without one.
struct Links {
    imports: Linked<FileLinks>,
    importers: Linked<FileLinks>,
    tests: Linked<FileLinks>,
}

impl Links {
    fn into_card(self, traced: FileTrace, more: Option<Section>) -> FileTrace {
        FileTrace {
            imports: wanted(more, Section::Imports).then_some(self.imports),
            importers: wanted(more, Section::Importers).then_some(self.importers),
            tests: wanted(more, Section::Tests).then_some(self.tests),
            ..traced
        }
    }
}

fn links(
    call: &Call<'_, '_, '_>,
    found: &FoundFile,
    more: Option<Section>,
) -> Noted<Result<Links, Error>> {
    let Some(anchor) = &found.anchor else {
        return Noted::new(Ok(Links {
            imports: Linked::NoAnchor,
            importers: Linked::NoAnchor,
            tests: Linked::NoAnchor,
        }));
    };
    let request = ExplainRequest {
        target: anchor.clone(),
        limit: IntArg::from_usize(MAX_LIMIT),
    };
    let skipped = &call.scan.skipped;
    explain_in(call.reading, &request, call.graph, skipped).map_ok(|explained| {
        let out = &explained.imports_out;
        let into = &explained.imports_in;
        Links {
            imports: Linked::Found(linked(
                out,
                |row| (&row.other, true),
                cap(more, Section::Imports, FILE_ROWS),
            )),
            importers: Linked::Found(linked(
                into,
                |row| (&row.path, !is_test_path(&row.path)),
                cap(more, Section::Importers, FILE_ROWS),
            )),
            tests: Linked::Found(linked(
                into,
                |row| (&row.path, is_test_path(&row.path)),
                cap(more, Section::Tests, FILE_ROWS),
            )),
        }
    })
}

// The distinct files that `side` gives for the rows it keeps, each at its first row's line.
fn linked(
    rows: &Bounded<ImportRow>,
    side: impl Fn(&ImportRow) -> (&String, bool),
    cap: usize,
) -> FileLinks {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut places = Vec::new();
    for row in rows.rows() {
        let (path, kept) = side(row);
        if kept && seen.insert(path) {
            places.push(Place {
                path: path.clone(),
                line: row.line,
            });
        }
    }
    let total = Tally::of(places.len(), rows.omitted() == 0);
    places.truncate(cap);
    FileLinks {
        rows: places,
        total,
    }
}

fn holding(all: &Bounded<crate::CycleRow>, path: &str, more: Option<Section>) -> Cycles {
    let mut rows: Vec<crate::CycleRow> = (all.rows().iter())
        .filter(|cycle| cycle.members.iter().any(|member| member == path))
        .cloned()
        .collect();
    let total = Tally::of(rows.len(), all.omitted() == 0);
    rows.truncate(cap(more, Section::Cycles, FILE_CYCLES));
    Cycles { rows, total }
}

// The folder's entries in tree order: each name at a level before the names below it.
pub(super) fn folder_trace(
    reading: &Reading<'_>,
    files: &[RepoFile],
    path: &str,
    more: Option<Section>,
) -> Noted<FolderTrace> {
    let (depth, entries) = if more == Some(Section::Tree) {
        (MORE_TREE_DEPTH, MORE_TREE_ENTRIES)
    } else {
        (FOLDER_DEPTH, FOLDER_ENTRIES)
    };
    let below: Vec<&str> = (files.iter())
        .filter_map(|file| below_folder(path, &file.path))
        .collect();
    let lines = (below.iter())
        .filter_map(|rest| read_lines(&reading.repo.root, &joined(path, rest)).ok())
        .map(u64::from)
        .sum();
    let tree = tree_entries(&below, depth);
    let mut noted = Noted::new(Vec::new());
    for (parts, node) in tree.iter().take(entries) {
        let entry = folder_entry(reading, path, parts, *node);
        noted.diagnostics.extend(entry.diagnostics);
        noted.value.push(entry.value);
    }
    noted.map(|kept| FolderTrace {
        path: path.to_owned(),
        files: below.len(),
        lines,
        entries: kept,
        total: tree.len(),
        depth,
    })
}

// The path of `file` below the folder at `path`, or `None` outside it; the root holds every file.
fn below_folder<'f>(path: &str, file: &'f str) -> Option<&'f str> {
    if path.is_empty() {
        return Some(file);
    }
    file.strip_prefix(path)?.strip_prefix('/')
}

fn joined(folder: &str, rest: &str) -> String {
    if folder.is_empty() {
        rest.to_owned()
    } else {
        format!("{folder}/{rest}")
    }
}

// One entry of the tree: a file, or a folder with the files below it.
#[derive(Clone, Copy)]
enum Node {
    File,
    Folder(usize),
}

// Each folder and file to `depth`, by its parts. Comparing parts puts a folder's entries right
// after it.
fn tree_entries(below: &[&str], depth: usize) -> BTreeMap<Vec<String>, Node> {
    let mut tree: BTreeMap<Vec<String>, Node> = BTreeMap::new();
    for rest in below {
        let parts: Vec<String> = rest.split('/').map(str::to_owned).collect();
        for level in 1..parts.len().min(depth + 1) {
            let folder = tree
                .entry(parts[..level].to_vec())
                .or_insert(Node::Folder(0));
            if let Node::Folder(files) = folder {
                *files += 1;
            }
        }
        if parts.len() <= depth {
            tree.insert(parts, Node::File);
        }
    }
    tree
}

fn folder_entry(
    reading: &Reading<'_>,
    folder: &str,
    parts: &[String],
    node: Node,
) -> Noted<FolderEntry> {
    let path = joined(folder, &parts.join("/"));
    let depth = parts.len();
    match node {
        Node::Folder(files) => Noted::new(FolderEntry::Folder { depth, path, files }),
        Node::File => file_row(reading, &path).map(|row| FolderEntry::File { depth, row }),
    }
}
