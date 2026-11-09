//! The tree side of `reuse_candidates`: the functions of the tree after the change with their
//! action names, from the tag cache or a parse, and the references between the two functions of
//! each proposed row.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use checks::{FUNCTION_REASONS, Link, NamedFunction, NamedTree, Outcome, Pair, Proposed};
use facts::{StableId, Symbol, SymbolKind};
use graph::{FileFacts, Relation, SymbolGraph};
use metrics::FileMetrics;
use syntax::Language;

use crate::rows::{bounded_read, decoded};
use crate::scan;
use crate::symbol_graph::with_resolved;
use crate::tree_scan::TreeScan;
use crate::{Error, Services, in_metric_scope};

// The manifest that names a Rust crate's root, which a partial graph needs to resolve `crate::`.
const RUST_MANIFEST: &str = "Cargo.toml";

// What the tag cache holds for one file's functions.
pub(crate) enum StoredUnits {
    // The stored symbols and measured units of this exact text.
    Hit(Vec<Symbol>, FileMetrics),
    // No usable row: parse the text in memory.
    Miss,
    // The rows did not read: parse the text in memory, and print this warning.
    Unreadable(String),
}

// The lookup of one file's stored symbols and units by path, text and language.
pub(crate) type UnitLookup<'a> = dyn FnMut(&str, &str, Language) -> StoredUnits + 'a;

// What the check reads besides its pairs: the repository, whether the form reads the tree from
// a tag cache that this run lacks, the lookup of its rows, and the warnings of the run.
pub(crate) struct ReuseSource<'a> {
    pub(crate) services: &'a Services,
    pub(crate) root: &'a Path,
    pub(crate) no_tag_cache: bool,
    pub(crate) units: &'a mut UnitLookup<'a>,
    pub(crate) warnings: &'a RefCell<Vec<String>>,
}

// --since resolves the rows' references on its scan's whole-tree graph. The other forms read the
// cache only, and their partial graph misses a reference through a re-export in another file.
pub(crate) fn reuse_candidates(
    pairs: &[&Pair],
    source: &mut ReuseSource<'_>,
    scan: Option<&TreeScan>,
) -> Result<Outcome, Error> {
    let tree = named_tree(pairs, source, scan.is_some())?;
    let proposed = checks::reuse_candidates(pairs, &tree)?;
    let Some(scan) = scan else {
        let linked = partial_links(pairs, source, &proposed)?;
        return Ok(proposed.outcome(&linked));
    };
    let stopped = || {
        let tree = NamedTree::Stopped {
            seconds: scan.seconds,
        };
        Ok(checks::reuse_candidates(pairs, &tree)?.outcome(&BTreeSet::new()))
    };
    let Some(repo) = &scan.facts else {
        return stopped();
    };
    let linked = with_resolved(&repo.files, Some(&scan.deadline), |_, graph| {
        Ok(linked_in(graph, proposed.functions(), &proposed.links()))
    });
    match linked {
        Ok(linked) => Ok(proposed.outcome(&linked)),
        Err(Error::Kind(kernel::Error::CallStopped(_))) => stopped(),
        Err(error) => Err(error),
    }
}

// Every function in scope of the tree after the change, in path and line order. The proposed
// form with no tag cache reads no tree, as its hook use requires.
fn named_tree(
    pairs: &[&Pair],
    source: &mut ReuseSource<'_>,
    since: bool,
) -> Result<NamedTree, Error> {
    if !since && source.no_tag_cache {
        return Ok(NamedTree::NoTagCache);
    }
    let started = Instant::now();
    let budget = source.services.walk_budget;
    let texts = tree_texts(pairs, source)?;
    let mut functions = Vec::new();
    let mut unread = BTreeMap::new();
    for (path, text) in &texts {
        if started.elapsed() >= budget {
            return Ok(NamedTree::Stopped {
                seconds: budget.as_secs(),
            });
        }
        let Some(language) = Language::from_path(path) else {
            continue;
        };
        let read = text
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|text| units_of(source, path, text, language));
        match read {
            Ok((symbols, metrics)) => {
                functions.extend(file_functions(path, language, &symbols, &metrics));
            }
            Err(reason) => {
                unread.insert(path.clone(), reason);
            }
        }
    }
    functions.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
    Ok(NamedTree::Built {
        functions,
        unread,
        files: texts.len(),
    })
}

type Texts = BTreeMap<String, Result<String, String>>;

// The text of each file in scope after the change: an edited file's new text, else its text on
// disk. A deleted file has none.
fn tree_texts(pairs: &[&Pair], source: &mut ReuseSource<'_>) -> Result<Texts, Error> {
    let walk = source.services.git.walk(source.root, scan::reads);
    let warnings = walk.warnings.iter().map(ToString::to_string);
    source.warnings.borrow_mut().extend(warnings);
    let files = walk
        .files
        .map_err(|refusal| Error::from(kernel::Error::from(refusal)))?;
    let edited: HashMap<&str, &Pair> = pairs.iter().map(|pair| (pair.path(), *pair)).collect();
    let mut texts = Texts::new();
    let paths = (files.iter().map(|file| file.path.as_str())).chain(edited.keys().copied());
    for path in paths.filter(|path| in_metric_scope(path)) {
        let text = match edited.get(path) {
            Some(pair) => match pair.new_text() {
                Some(text) => Ok(text.to_owned()),
                None => continue,
            },
            None => decoded(bounded_read(&source.root.join(path))),
        };
        texts.insert(path.to_owned(), text);
    }
    Ok(texts)
}

// The stored symbols and units of a text, else one parse of it.
fn units_of(
    source: &mut ReuseSource<'_>,
    path: &str,
    text: &str,
    language: Language,
) -> Result<(Vec<Symbol>, FileMetrics), String> {
    match (source.units)(path, text, language) {
        StoredUnits::Hit(symbols, metrics) => return Ok((symbols, metrics)),
        StoredUnits::Unreadable(warning) => source.warnings.borrow_mut().push(warning),
        StoredUnits::Miss => {}
    }
    let parsed =
        syntax::parse(language, Arc::from(text)).map_err(|unparsable| unparsable.to_string())?;
    let metrics = metrics::measure(&parsed).map_err(|error| {
        (error.reason(path, &FUNCTION_REASONS)).unwrap_or_else(|| error.to_string())
    })?;
    Ok((facts::extract(&parsed).symbols, metrics))
}

// Each function or method symbol joined to its metrics unit: the same own name, and the unit's
// first line on the symbol's name line.
fn file_functions(
    path: &str,
    language: Language,
    symbols: &[Symbol],
    metrics: &FileMetrics,
) -> Vec<NamedFunction> {
    let units: HashMap<(&str, u32), usize> = (metrics.units.iter().enumerate())
        .map(|(at, unit)| ((own_name(&unit.qualname), unit.start_line), at))
        .collect();
    let functions = symbols
        .iter()
        .filter(|symbol| matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method));
    functions
        .filter_map(|symbol| {
            let at = *units.get(&(symbol.name.as_str(), symbol.name_line))?;
            Some(NamedFunction {
                id: StableId::of(language, path, symbol).to_string(),
                path: path.to_owned(),
                language,
                qualname: metrics.units[at].qualname.clone(),
                line: symbol.line,
                name_line: symbol.name_line,
                protocol: symbol.marks.protocol,
                test: symbol.marks.test,
                names: metrics.actions[at].clone(),
            })
        })
        .collect()
}

// A metrics qualname joins its scopes with dots, so the own name is its last part.
fn own_name(qualname: &str) -> &str {
    qualname.rsplit_once('.').map_or(qualname, |(_, name)| name)
}

// The graph of the edited files' new texts, the files of the proposed rows, and the Rust
// manifests of the tree, parsed in memory.
fn partial_links(
    pairs: &[&Pair],
    source: &ReuseSource<'_>,
    proposed: &Proposed<'_>,
) -> Result<BTreeSet<Link>, Error> {
    let links = proposed.links();
    if links.is_empty() {
        return Ok(BTreeSet::new());
    }
    let functions = proposed.functions();
    let edited: HashMap<&str, &str> = (pairs.iter())
        .filter_map(|pair| Some((pair.path(), pair.new_text()?)))
        .collect();
    let mut paths: BTreeSet<&str> = edited.keys().copied().collect();
    for (unit, other) in &links {
        paths.insert(&functions[*unit].path);
        paths.insert(&functions[*other].path);
    }
    let walk = source.services.git.walk(source.root, scan::reads);
    let walked = walk.files.iter().flatten().map(|file| file.path.as_str());
    paths.extend(walked.filter(|path| path.rsplit('/').next() == Some(RUST_MANIFEST)));
    let files: Vec<FileFacts> = (paths.into_iter())
        .filter_map(|path| {
            let text = match edited.get(path) {
                Some(text) => (*text).to_owned(),
                None => decoded(bounded_read(&source.root.join(path))).ok()?,
            };
            parsed_facts(path, &text)
        })
        .collect();
    with_resolved(&files, None, |_, graph| {
        Ok(linked_in(graph, functions, &links))
    })
}

fn parsed_facts(path: &str, text: &str) -> Option<FileFacts> {
    scan::parsed_file(path, Language::from_path(path)?, text)
}

// The links whose two functions reference each other, either way, at a binding tier.
fn linked_in(
    graph: &SymbolGraph<'_>,
    functions: &[NamedFunction],
    links: &[Link],
) -> BTreeSet<Link> {
    let references = |from: &NamedFunction, to: &NamedFunction| {
        (graph.outgoing(&from.id).iter()).any(|edge| {
            edge.relation() == Relation::References
                && edge.tier().binds()
                && edge.target().node() == to.id
        })
    };
    (links.iter().copied())
        .filter(|(unit, other)| {
            let (unit, other) = (&functions[*unit], &functions[*other]);
            references(unit, other) || references(other, unit)
        })
        .collect()
}
