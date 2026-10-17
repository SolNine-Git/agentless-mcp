//! The repository map: the files the reference graph ranks highest for the focus, the symbols
//! under them that fit the token budget, and the tests that exercise them.

mod body;
mod churn;
mod companions;
mod focus;
mod pack;
mod text;

use std::collections::{BTreeMap, BTreeSet};

use ::text::strip;
use graph::{Definition, FileFacts, NameIndex, PageRank, Scopes, pagerank, rank_order};
use io::{Granularity, ProjectConfig};

use crate::bounds::within;
use crate::detached::answered_over;
use crate::file_graph::file_graph;
use crate::ops::limits::{DEFAULT_MAX_FILES, MAX_BUDGET, MAX_MAX_FILES, MIN_BUDGET, MIN_MAX_FILES};
use crate::scan::RepoFacts;
use crate::scope::is_test_path;
use crate::{Error, IntArg, Noted, RationaleNode, Reading, Services, Skipped};

pub use body::{BodyMapResult, body_map};
pub use companions::{Companion, Companions};
pub(crate) use focus::{Known, focus_resolution};
pub use text::map_rows_text;

/// A map request: the focus, the token budget, the file cap and the granularity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapRequest {
    /// The files and symbols that seed the ranking.
    pub focus: Vec<String>,
    /// The token budget of the symbols, or `None` to size it from the repository.
    pub budget: Option<IntArg>,
    /// The most ranked files the map lists, or `None` for the config's cap.
    pub max_files: Option<IntArg>,
    /// The map's detail, or `None` for the config's.
    pub granularity: Option<Granularity>,
}

/// The ranked files with the symbols that fit, the counts behind them, and the test companions.
#[derive(Clone, Debug, PartialEq)]
pub struct MapResult {
    /// The ranked files, best first.
    pub files: Vec<MapFile>,
    /// The token budget of the symbols; 0 for a map of files alone.
    pub budget: usize,
    /// The symbols the budget kept.
    pub included: usize,
    /// The symbols that competed for the budget: those of the files the focus reached.
    pub candidates: usize,
    /// The files the ranking scored.
    pub ranked: usize,
    /// The tokens of the rendered rows.
    pub rendered: usize,
    /// Whether the ranking's iteration settled.
    pub rank_converged: bool,
    /// The seeded files, sorted.
    pub seeds: Vec<String>,
    /// The focus entries that seeded nothing.
    pub unresolved_seeds: Vec<String>,
    /// The files the scan passed over.
    pub skipped: Vec<Skipped>,
    /// The kept symbols' stable ids in score order.
    pub expand_order: Vec<String>,
    /// The stable ids of the symbols the focus named, in the ranking's file order.
    pub focus_order: Vec<String>,
    /// The test files that exercise the ranked and seeded files.
    pub companions: Companions,
}

/// One ranked file, with the symbols of it that fit.
#[derive(Clone, Debug, PartialEq)]
pub struct MapFile {
    /// The path below the repository root.
    pub path: String,
    /// The file's rank.
    pub rank: f64,
    /// The symbols the map lists, by line.
    pub entries: Vec<MapEntry>,
    /// The file's symbols that competed for a place.
    pub total: usize,
    /// The language prefix of the entries' stable ids, or empty when there is no entry.
    pub id_prefix: &'static str,
    /// Whether the ranking's walk reached the file from the focus.
    pub reached: bool,
    /// The commits inside the churn window, or `None` when git could not answer.
    pub commits_window: Option<usize>,
    /// The days since the newest of them, or `None` when there is none or git could not answer.
    pub last_commit_days: Option<i64>,
}

impl MapFile {
    /// Returns how many of the file's symbols the map leaves out.
    #[must_use]
    pub fn omitted(&self) -> usize {
        self.total.saturating_sub(self.entries.len())
    }
}

/// One symbol row of a map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapEntry {
    /// The symbol's first line.
    pub line: u32,
    /// The one-line signature.
    pub signature: String,
    /// The stable id.
    pub stable_id: String,
    /// 1 for a symbol inside a class, else 0.
    pub depth: usize,
    /// The rationale nodes linked to the symbol.
    pub rationales: Vec<RationaleNode>,
    /// The qualified name that the stable id addresses the symbol by.
    pub name: String,
}

/// Ranks the files of the repository that `reading` names, packs the symbols that fit the
/// budget, and finds the tests that exercise them. The scan's warnings come with either outcome.
#[must_use]
pub fn map(
    services: &Services,
    reading: &Reading<'_>,
    request: &MapRequest,
) -> Noted<Result<MapResult, Error>> {
    let shape = match Shape::of(request, &reading.repo.config) {
        Ok(shape) => shape,
        Err(error) => return Noted::new(Err(error)),
    };
    let loaded = RepoFacts::load(services, reading);
    loaded.map(|facts| {
        facts.map(|facts| {
            answered_over(facts, |facts| {
                build(services, reading, &request.focus, &shape, facts)
            })
        })
    })
}

// The request's bounds, checked before the scan, with the config's values filled in.
struct Shape {
    max_files: usize,
    budget: Option<usize>,
    granularity: Granularity,
}

impl Shape {
    fn of(request: &MapRequest, config: &ProjectConfig) -> Result<Shape, Error> {
        let configured = config
            .max_files
            .map(|files| IntArg::from_usize(files as usize));
        let max_files = (request.max_files.clone().or(configured))
            .unwrap_or_else(|| IntArg::from_usize(DEFAULT_MAX_FILES));
        let max_files = within(&max_files, MIN_MAX_FILES, MAX_MAX_FILES, "max_files")?;
        let budget = (request.budget.as_ref())
            .map(|budget| within(budget, MIN_BUDGET, MAX_BUDGET, "budget"))
            .transpose()?;
        let granularity = request.granularity.or(config.granularity);
        Ok(Shape {
            max_files,
            budget,
            granularity: granularity.unwrap_or(Granularity::Function),
        })
    }
}

// The ranking's chosen files, best first, with their facts and ranks.
struct Shortlist<'r, 'a> {
    chosen: &'r [&'a str],
    ranking: &'r PageRank<'a>,
    by_path: &'r BTreeMap<&'a str, &'a FileFacts>,
}

impl<'a> Shortlist<'_, 'a> {
    // Every chosen file came off this scan's ranking, so a missing one is a defect.
    fn rank(&self, path: &str) -> f64 {
        (self.ranking.rank(path)).expect("a chosen file is a ranked file")
    }

    fn file(&self, path: &str) -> &'a FileFacts {
        self.by_path
            .get(path)
            .expect("a chosen file is a scanned file")
    }
}

// The files of one granularity, with the packing's counts and orders.
struct Listing {
    files: Vec<MapFile>,
    budget: usize,
    included: usize,
    candidates: usize,
    expand_order: Vec<String>,
    focus_order: Vec<String>,
}

fn build(
    services: &Services,
    reading: &Reading<'_>,
    focus: &[String],
    shape: &Shape,
    facts: &RepoFacts,
) -> MapResult {
    let config = &reading.repo.config;
    let index = NameIndex::build(&facts.files);
    let graph = file_graph(&index, &Scopes::build(&facts.files), config);
    let known: Known<'_> = facts.files.iter().map(|file| file.path.as_str()).collect();
    let seeding = seed_weights(focus, &known, &index);
    // Tests stay pure sources of rank: the companions are their channel into a map.
    let ranking = pagerank(&graph, &seeding.weights, is_test_path);
    let mut chosen = rank_order(&ranking, |path| seeding.weights.contains_key(path));
    chosen.truncate(shape.max_files);
    let by_path: BTreeMap<&str, &FileFacts> = (facts.files.iter())
        .map(|file| (file.path.as_str(), file))
        .collect();
    let seeded = seeding.weights.keys().map(String::as_str);
    let targets: BTreeSet<&str> = seeded.chain(chosen.iter().copied()).collect();
    let companions = companions::companions_for(&graph, &by_path, &index, &targets);
    let ranked = Shortlist {
        chosen: &chosen,
        ranking: &ranking,
        by_path: &by_path,
    };
    let listing = match shape.granularity {
        Granularity::File => file_listing(&ranked),
        Granularity::Function => {
            let symbols = pack::Symbols {
                index: &index,
                stoplist: &config.stoplist,
                definitions: &seeding.definitions,
            };
            pack::symbol_listing(&*services.counter, &ranked, &symbols, shape.budget)
        }
    };
    let files = churn::with_churn(services, reading, listing.files);
    MapResult {
        rendered: services.counter.count(&map_rows_text(&files)),
        files,
        budget: listing.budget,
        included: listing.included,
        candidates: listing.candidates,
        ranked: ranking.ranks().count(),
        rank_converged: ranking.converged(),
        seeds: seeding.weights.keys().cloned().collect(),
        unresolved_seeds: seeding.unresolved,
        skipped: facts.skipped.clone(),
        expand_order: listing.expand_order,
        focus_order: listing.focus_order,
        companions,
    }
}

// No symbol competes in a map of files, so the counts are 0; each row keeps its file's count.
fn file_listing(ranked: &Shortlist<'_, '_>) -> Listing {
    let files = (ranked.chosen.iter())
        .map(|&path| MapFile {
            path: path.to_owned(),
            rank: ranked.rank(path),
            entries: Vec::new(),
            total: ranked.file(path).facts.symbols.len(),
            id_prefix: "",
            reached: ranked.ranking.reached(path),
            commits_window: None,
            last_commit_days: None,
        })
        .collect();
    Listing {
        files,
        budget: 0,
        included: 0,
        candidates: 0,
        expand_order: Vec::new(),
        focus_order: Vec::new(),
    }
}

// The ranking's seed weights, the focus entries that seeded nothing, and the definitions that the
// symbol entries named.
struct Seeding<'a> {
    weights: BTreeMap<String, f64>,
    unresolved: Vec<String>,
    definitions: Vec<Definition<'a>>,
}

// One entry, one vote: an entry's mass is split across the files it resolved to.
fn seed_weights<'a>(focus: &[String], known: &Known<'_>, index: &NameIndex<'a>) -> Seeding<'a> {
    let mut seeding = Seeding {
        weights: BTreeMap::new(),
        unresolved: Vec::new(),
        definitions: Vec::new(),
    };
    for entry in focus.iter().map(String::as_str).map(strip) {
        if entry.is_empty() {
            continue;
        }
        let (paths, matched) = focus_resolution(entry, known, index);
        if paths.is_empty() {
            if !seeding.unresolved.iter().any(|seen| seen == entry) {
                seeding.unresolved.push(entry.to_owned());
            }
            continue;
        }
        seeding.definitions.extend(matched);
        let share = 1.0 / count(paths.len());
        for path in paths {
            *seeding.weights.entry(path).or_insert(0.0) += share;
        }
    }
    seeding
}

// A count of files or sites; a scan holds fewer than 2^32 of either.
fn count(items: usize) -> f64 {
    u32::try_from(items)
        .map(f64::from)
        .expect("a scan holds fewer than 2^32 files or sites")
}

#[cfg(test)]
mod tests;
