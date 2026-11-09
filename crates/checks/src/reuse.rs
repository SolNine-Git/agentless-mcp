//! `reuse_candidates`: each function a change adds whose action names (the names it calls and
//! the fields it reads) are mostly the names of an existing function, by IDF-weighted Jaccard.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use prompts::messages;
use syntax::Language;

use crate::callees::own_name;
use crate::error::Error;
use crate::finding::{
    Detail, Finding, Match, NameShare, PairFinding, Severity, budget_spent, not_checked, sorted,
};
use crate::matching::{Paired, file_units, match_run};
use crate::observed::{Observed, Outcome};
use crate::pair::{Pair, REUSE_GAPS, each_pair, read_sides};

// The filters and limits of the reuse pass, as data with one home.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ReuseRules {
    // The fewest action names that a function needs on either side of a row.
    min_names: usize,
    // A name counts toward a match only in at most this share of all functions.
    common_share: f64,
    // The lowest score that a row shows.
    floor: f64,
    // The most rows that one added function gives.
    rows_per_function: usize,
}

// The rules in force. No held-out label set calibrates the floor, so the check stays opt-in.
pub(crate) const REUSE_RULES: ReuseRules = ReuseRules {
    min_names: 3,
    common_share: 0.05,
    floor: 0.3,
    rows_per_function: 2,
};

/// One function of the tree after the change, with its action names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedFunction {
    /// The stable id.
    pub id: String,
    /// The repo-relative path.
    pub path: String,
    /// The language of the file.
    pub language: Language,
    /// The qualified name, with dots between its parts.
    pub qualname: String,
    /// The first line, at the first attribute or decorator.
    pub line: u32,
    /// The line of the name, which is the first line of the function's metrics unit.
    pub name_line: u32,
    /// A protocol slot: a Rust method of `impl Trait for T`, or a Python dunder.
    pub protocol: bool,
    /// Rust test code.
    pub test: bool,
    /// The names that it calls and the fields that it reads, each once, sorted.
    pub names: Vec<String>,
}

/// The functions that the check reads in the tree after the change, or why it has none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamedTree {
    /// Every function in the check's scope, and the files that did not read with the reason.
    Built {
        /// The functions, in path and line order.
        functions: Vec<NamedFunction>,
        /// The repository files in scope that gave no functions, each with its reason.
        unread: BTreeMap<String, String>,
        /// The count of repository files in scope.
        files: usize,
    },
    /// The hook form reads the tree from the tag cache only, and this run has none.
    NoTagCache,
    /// The read of the tree reached the time budget of `seconds`.
    Stopped {
        /// The time budget, in whole seconds.
        seconds: u64,
    },
}

/// One row before the edge filter: the index of the added function and of the existing one.
pub type Link = (usize, usize);

/// The rows of a run before the edge filter, and the rows that say why a file was not checked.
pub struct Proposed<'t> {
    index: Index<'t>,
    findings: Vec<PairFinding>,
    scored: Vec<Scored>,
}

impl Proposed<'_> {
    /// Returns each row before the edge filter, which the caller tests for a reference between
    /// its two functions.
    #[must_use]
    pub fn links(&self) -> Vec<Link> {
        self.scored
            .iter()
            .map(|row| (row.unit, row.other))
            .collect()
    }

    /// Returns the functions that a link's indices name.
    #[must_use]
    pub fn functions(&self) -> &[NamedFunction] {
        self.index.functions
    }

    /// Returns the outcome with the rows of `linked` left out: their two functions reference
    /// each other, so one uses the other and is no copy of it.
    #[must_use]
    pub fn outcome(self, linked: &BTreeSet<Link>) -> Outcome {
        let mut findings = self.findings;
        let kept = self.index.kept_rows(self.scored, linked);
        findings.extend(kept.iter().map(|row| finding(&self.index, row)));
        Outcome {
            findings: sorted(findings),
            observed: Observed::default(),
        }
    }
}

/// Propose a row for each function that a change adds whose action names match those of an
/// existing function of `tree` at the floor or above, best first.
///
/// # Errors
///
/// Returns an error when two pairs name one path, or when a side fails in a way that is not a
/// parse or tokenizer failure.
pub fn reuse_candidates<'t>(pairs: &[&Pair], tree: &'t NamedTree) -> Result<Proposed<'t>, Error> {
    let (functions, unread, files) = match tree {
        NamedTree::Built {
            functions,
            unread,
            files,
        } => (functions.as_slice(), unread, *files),
        NamedTree::NoTagCache => return Ok(only(no_tag_cache())),
        NamedTree::Stopped { seconds } => {
            let (reads, compared) = (
                messages::REUSE_CANDIDATES_READS,
                messages::REUSE_CANDIDATES_COMPARED,
            );
            let row = budget_spent(crate::REUSE_CANDIDATES, reads, compared, *seconds);
            return Ok(only(row));
        }
    };
    let (measured, mut findings) = each_pair(pairs, |pair, language| {
        read_sides(pair, language, &REUSE_GAPS)
    })?;
    let matches = match_run(&file_units(&measured));
    let index = Index::new(functions, &REUSE_RULES);
    let added: Vec<usize> = (matches.iter())
        .filter(|paired| paired.via == Match::New)
        .filter_map(|paired| index.place_of(paired))
        .filter(|at| index.kept[*at])
        .collect();
    let scored = added.iter().flat_map(|at| index.scored(*at)).collect();
    findings.extend(unread_gaps(unread, files));
    Ok(Proposed {
        index,
        findings,
        scored,
    })
}

fn only(row: PairFinding) -> Proposed<'static> {
    Proposed {
        index: Index::new(&[], &REUSE_RULES),
        findings: vec![row],
        scored: Vec::new(),
    }
}

// One scored row: the added function, the existing one, the score and the shared names.
struct Scored {
    unit: usize,
    other: usize,
    score: f64,
    shared: Vec<usize>,
}

// The names of every function as ids, their document frequencies, and the postings of the names
// that count toward a match.
struct Index<'f> {
    functions: &'f [NamedFunction],
    rules: &'f ReuseRules,
    by_place: HashMap<(&'f str, &'f str, u32), usize>,
    names: Vec<&'f str>,
    of: Vec<Vec<usize>>,
    idf: Vec<f64>,
    weight: Vec<f64>,
    kept: Vec<bool>,
    postings: HashMap<usize, Vec<usize>>,
}

impl<'f> Index<'f> {
    fn new(functions: &'f [NamedFunction], rules: &'f ReuseRules) -> Index<'f> {
        let (names, of) = name_ids(functions);
        let mut df = vec![0usize; names.len()];
        for name in of.iter().flatten() {
            df[*name] += 1;
        }
        let total = count(functions.len());
        let idf: Vec<f64> = (df.iter())
            .map(|&df| ((total + 1.0) / (count(df) + 1.0)).ln() + 1.0)
            .collect();
        let weight = (of.iter())
            .map(|names| names.iter().map(|name| idf[*name]).sum())
            .collect();
        let kept: Vec<bool> = (functions.iter())
            .map(|function| {
                function.names.len() >= rules.min_names && !function.protocol && !function.test
            })
            .collect();
        let limit = total * rules.common_share;
        let mut postings: HashMap<usize, Vec<usize>> = HashMap::new();
        for (at, names) in of.iter().enumerate().filter(|(at, _)| kept[*at]) {
            for name in names.iter().filter(|name| count(df[**name]) <= limit) {
                postings.entry(*name).or_default().push(at);
            }
        }
        let by_place = (functions.iter().enumerate())
            .map(|(at, function)| {
                let own = own_name(&function.qualname);
                ((function.path.as_str(), own, function.name_line), at)
            })
            .collect();
        Index {
            functions,
            rules,
            by_place,
            names,
            of,
            idf,
            weight,
            kept,
            postings,
        }
    }

    // The tree function of an added unit: the same path, own name, and name line.
    fn place_of(&self, paired: &Paired<'_>) -> Option<usize> {
        let unit = paired.new.unit;
        let place = (paired.new.path, own_name(&unit.qualname), unit.start_line);
        self.by_place.get(&place).copied()
    }

    // Each kept function of the same language that shares a counted name with `unit` and scores
    // at the floor or above: the counted shared weight over the weight of the union.
    fn scored(&self, unit: usize) -> Vec<Scored> {
        let language = self.functions[unit].language;
        let mut shared: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        let compared =
            |other: &&usize| **other != unit && self.functions[**other].language == language;
        for name in &self.of[unit] {
            for &other in self
                .postings
                .get(name)
                .into_iter()
                .flatten()
                .filter(compared)
            {
                shared.entry(other).or_default().push(*name);
            }
        }
        (shared.into_iter())
            .map(|(other, names)| {
                let common: f64 = names.iter().map(|name| self.idf[*name]).sum();
                let union = self.weight[unit] + self.weight[other] - common;
                Scored {
                    unit,
                    other,
                    score: common / union,
                    shared: names,
                }
            })
            .filter(|row| row.score >= self.rules.floor)
            .collect()
    }

    // Each added function keeps its best rows that do not join two linked functions. Two added
    // functions that each list the other keep the row of the one that comes first.
    fn kept_rows(&self, scored: Vec<Scored>, linked: &BTreeSet<Link>) -> Vec<Scored> {
        let mut by_unit: BTreeMap<usize, Vec<Scored>> = BTreeMap::new();
        for row in scored {
            if !linked.contains(&(row.unit, row.other)) {
                by_unit.entry(row.unit).or_default().push(row);
            }
        }
        let mut kept: Vec<Scored> = Vec::new();
        for rows in by_unit.values_mut() {
            rows.sort_by(|a, b| self.rank(a, b));
            kept.extend(rows.drain(..).take(self.rules.rows_per_function));
        }
        let found: BTreeSet<Link> = kept.iter().map(|row| (row.unit, row.other)).collect();
        let place = |at: usize| (self.functions[at].path.as_str(), self.functions[at].line);
        kept.retain(|row| {
            !found.contains(&(row.other, row.unit)) || place(row.unit) < place(row.other)
        });
        kept
    }

    // The higher score first, then the other function's path and line.
    fn rank(&self, a: &Scored, b: &Scored) -> Ordering {
        let place = |row: &Scored| {
            let other = &self.functions[row.other];
            (other.path.as_str(), other.line)
        };
        b.score
            .total_cmp(&a.score)
            .then_with(|| place(a).cmp(&place(b)))
    }
}

// Each distinct name once, in first-use order, and each function's names as their ids.
fn name_ids(functions: &[NamedFunction]) -> (Vec<&str>, Vec<Vec<usize>>) {
    let mut ids: HashMap<&str, usize> = HashMap::new();
    let mut names: Vec<&str> = Vec::new();
    let mut of = Vec::with_capacity(functions.len());
    for function in functions {
        let mut held = Vec::with_capacity(function.names.len());
        for name in &function.names {
            let id = *ids.entry(name.as_str()).or_insert(names.len());
            if id == names.len() {
                names.push(name.as_str());
            }
            held.push(id);
        }
        of.push(held);
    }
    (names, of)
}

fn finding(index: &Index<'_>, row: &Scored) -> PairFinding {
    let (unit, other) = (&index.functions[row.unit], &index.functions[row.other]);
    let mut shared: Vec<&str> = row.shared.iter().map(|name| index.names[*name]).collect();
    shared.sort_unstable();
    let listed = (shared.iter())
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let message = messages::REUSE_CANDIDATES_FOUND.render((
        other.path.as_str(),
        other.qualname.as_str(),
        unit.qualname.as_str(),
        other.line as usize,
        shared.len(),
        listed.as_str(),
    ));
    let score = format!("{:.3}", row.score);
    let evidence = messages::REUSE_CANDIDATES_FOUND_EVIDENCE.render(score.as_str());
    let detail = NameShare {
        unit: unit.qualname.clone(),
        other: other.qualname.clone(),
        other_path: other.path.clone(),
        other_line: other.line,
        score,
        shared: shared.iter().map(|name| (*name).to_owned()).collect(),
    };
    PairFinding {
        finding: Finding {
            check: crate::REUSE_CANDIDATES,
            severity: Severity::Advisory,
            message,
            path: unit.path.clone(),
            line: unit.name_line,
            evidence,
        },
        detail: Some(Detail::Reuse(Box::new(detail))),
    }
}

fn no_tag_cache() -> PairFinding {
    let message = messages::REUSE_CANDIDATES_NO_TAG_CACHE.to_owned();
    let evidence = messages::REUSE_CANDIDATES_NO_TAG_CACHE_EVIDENCE;
    not_checked(crate::REUSE_CANDIDATES, "", message, evidence, None)
}

// One row for the files that gave no functions, which names the first with its reason.
fn unread_gaps(unread: &BTreeMap<String, String>, files: usize) -> Option<PairFinding> {
    let (path, reason) = unread.iter().next()?;
    let paths: Vec<&str> = unread.keys().map(String::as_str).collect();
    let message =
        messages::REUSE_CANDIDATES_GAPS.render((paths.join(", ").as_str(), unread.len(), files));
    let evidence = messages::REUSE_CANDIDATES_GAPS_EVIDENCE.render((path.as_str(), reason));
    Some(not_checked(
        crate::REUSE_CANDIDATES,
        "",
        message,
        &evidence,
        None,
    ))
}

// A count of functions or names is far below 2^52, so it is exact as an f64.
fn count(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests;
