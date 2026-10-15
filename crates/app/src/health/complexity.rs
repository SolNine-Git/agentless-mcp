//! The complexity walk of health: each tier-1 source file outside test and fixture paths,
//! measured in path order until the time budget runs out, and the figures read off the walk.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use facts::SymbolKind;
use graph::Definition;
use kernel::{Bounded, Key};
use metrics::{Block, FileMetrics, MIN_INSTANCES, Unit, compensated_sum};
use store::{FactSource, FileText, Served};
use syntax::Language;

use crate::parallel::{map_in_order, threads};
use crate::scan::RepoFacts;
use crate::{Noted, Reading, Services, Skipped, in_metric_scope};

/// The complexity summary over the files one walk measured.
#[derive(Clone, Debug, PartialEq)]
pub struct HealthComplexity {
    /// A unit counts toward erosion when its cc is above this.
    pub threshold: u32,
    /// The files in scope.
    pub files: usize,
    /// The files the walk reached before the budget ran out.
    pub walked: usize,
    /// The units measured.
    pub units: usize,
    /// The units with a cc above the threshold.
    pub over_threshold: usize,
    /// The share of all mass that the units above the threshold hold.
    pub erosion: f64,
    /// The source lines of the measured files.
    pub sloc: usize,
    /// The source lines inside an instance of a Type-2 clone group.
    pub clone_lines: usize,
    /// The walked files that gave no metrics, with the reason, cut at the limit.
    pub not_measured: Bounded<Skipped>,
}

impl HealthComplexity {
    /// Returns how many walked files gave metrics.
    #[must_use]
    pub fn measured(&self) -> usize {
        self.walked.saturating_sub(self.not_measured.total())
    }

    /// Returns the share of the measured source lines inside a clone instance, 0.0 for none.
    #[must_use]
    pub fn clone_share(&self) -> f64 {
        if self.sloc == 0 {
            return 0.0;
        }
        count_f64(self.clone_lines) / count_f64(self.sloc)
    }

    /// Returns true when the time budget ended the walk before every file in scope.
    #[must_use]
    pub fn stopped_by_budget(&self) -> bool {
        self.walked < self.files
    }
}

/// One measured unit among the hotspots, with its stable id when exactly one symbol matches it.
#[derive(Clone, Debug, PartialEq)]
pub struct HealthComplexityRow {
    /// The path below the repository root.
    pub path: String,
    /// The first line.
    pub line: u32,
    /// The unit's qualified name.
    pub label: String,
    /// The cyclomatic complexity.
    pub cc: u32,
    /// The source lines.
    pub sloc: u32,
    /// cc times the square root of sloc.
    pub mass: f64,
    /// The stable id of the one callable symbol that matches, or `None`.
    pub stable_id: Option<String>,
}

// What one walk measured, what it could not measure, and how far it got.
#[derive(Debug, Default)]
pub(super) struct Walk {
    pub(super) units: Vec<PlacedUnit>,
    files: Vec<(String, FileMetrics)>,
    not_measured: Vec<Skipped>,
    walked: usize,
    in_scope: usize,
}

#[derive(Debug)]
pub(super) struct PlacedUnit {
    path: String,
    unit: Unit,
}

impl PlacedUnit {
    fn mass(&self) -> f64 {
        mass(&self.unit)
    }
}

// One file in scope: a parsed file, or a file the scan passed over with its reason.
struct Target {
    path: String,
    language: Language,
    skipped: Option<String>,
}

// The budget is checked before each file, so a spent budget ends the walk and never the call.
pub(super) fn walk(services: &Services, reading: &Reading<'_>, facts: &RepoFacts) -> Noted<Walk> {
    let targets = targets(facts);
    let (clock, budget) = (&*services.clock, services.walk_budget);
    let started = clock.now();
    let measured = map_in_order(&targets, threads(), |target| {
        let within = clock.now().duration_since(started) < budget;
        within.then(|| measure(&reading.repo.root, target, reading.facts))
    });
    walked(&targets, measured)
}

// The files measured before the first file that the budget turned away, in path order. A cut walk
// keeps a prefix, as a serial walk does.
fn walked(
    targets: &[Target],
    measured: Vec<Option<Served<Result<FileMetrics, Skipped>>>>,
) -> Noted<Walk> {
    let mut walked = Noted::new(Walk {
        in_scope: targets.len(),
        ..Walk::default()
    });
    let kept = (targets.iter().zip(measured)).map_while(|(target, served)| Some((target, served?)));
    for (target, Served { value, warning }) in kept {
        walked
            .diagnostics
            .extend(warning.map(|failure| failure.to_string()));
        record(&mut walked.value, &target.path, value);
    }
    walked
}

fn record(walk: &mut Walk, path: &str, measured: Result<FileMetrics, Skipped>) {
    match measured {
        Ok(mut metrics) => {
            let units = std::mem::take(&mut metrics.units);
            walk.units.extend(units.into_iter().map(|unit| PlacedUnit {
                path: path.to_owned(),
                unit,
            }));
            walk.files.push((path.to_owned(), metrics));
        }
        Err(skipped) => walk.not_measured.push(skipped),
    }
    walk.walked += 1;
}

// A scan skip is a file in scope too: a walk over the parsed files alone would drop it unsaid.
fn targets(facts: &RepoFacts) -> Vec<Target> {
    let parsed = facts.files.iter().map(|file| Target {
        path: file.path.clone(),
        language: file.language,
        skipped: None,
    });
    let skipped = facts.skipped.iter().filter_map(|entry| {
        Some(Target {
            path: entry.path.clone(),
            language: Language::from_path(&entry.path)?,
            skipped: Some(entry.reason.clone()),
        })
    });
    let mut targets: Vec<Target> = parsed
        .chain(skipped)
        .filter(|target| in_metric_scope(&target.path))
        .collect();
    targets.sort_by(|left, right| left.path.cmp(&right.path));
    targets
}

// One failure drops the file from both figures, so erosion and the clone share cover one set.
fn measure(
    root: &Path,
    target: &Target,
    source: &FactSource,
) -> Served<Result<FileMetrics, Skipped>> {
    let skip = |reason: String| Skipped {
        path: target.path.clone(),
        reason,
    };
    let unserved = |value| Served {
        value,
        warning: None,
    };
    if let Some(reason) = &target.skipped {
        return unserved(Err(skip(reason.clone())));
    }
    let bytes = match io::read_bounded(&root.join(&target.path)) {
        Ok(bytes) => bytes,
        Err(skipped) => return unserved(Err(skip(skipped.to_string()))),
    };
    let text = FileText::decode(target.path.clone(), target.language, &bytes);
    let Served { value, warning } = source.metrics(&text);
    let value = value.map_err(|unmeasured| skip(unmeasured.reason));
    Served { value, warning }
}

pub(super) fn summary(walk: &Walk, threshold: u32, limit: usize) -> HealthComplexity {
    let masses: Vec<f64> = walk.units.iter().map(PlacedUnit::mass).collect();
    let over: Vec<f64> = (walk.units.iter())
        .filter(|placed| placed.unit.cc > threshold)
        .map(PlacedUnit::mass)
        .collect();
    HealthComplexity {
        threshold,
        files: walk.in_scope,
        walked: walk.walked,
        units: walk.units.len(),
        over_threshold: over.len(),
        erosion: erosion(&masses, &over),
        sloc: walk
            .files
            .iter()
            .map(|(_, file)| file.sloc_lines.len())
            .sum(),
        clone_lines: clone_lines(&walk.files),
        not_measured: Bounded::cut(walk.not_measured.clone(), limit),
    }
}

fn mass(unit: &Unit) -> f64 {
    f64::from(unit.cc) * f64::from(unit.sloc).sqrt()
}

// The share of all mass that the units above the threshold hold, 0.0 when there is no mass.
fn erosion(masses: &[f64], over: &[f64]) -> f64 {
    let total = compensated_sum(masses);
    if total == 0.0 {
        return 0.0;
    }
    compensated_sum(over) / total
}

// Each file's source lines inside any block whose Type-2 key at least two blocks share.
fn clone_lines(files: &[(String, FileMetrics)]) -> usize {
    let mut instances: BTreeMap<Key, usize> = BTreeMap::new();
    for (_, file) in files {
        for block in &file.blocks {
            *instances.entry(block.key).or_default() += 1;
        }
    }
    let grouped = |block: &&Block| instances.get(&block.key).copied().unwrap_or(0) >= MIN_INSTANCES;
    let lines_of = |file: &FileMetrics| -> usize {
        let blocks = file.blocks.iter().filter(grouped);
        let inside = blocks.flat_map(|block| lines_within(&file.sloc_lines, block));
        inside.copied().collect::<BTreeSet<u32>>().len()
    };
    files.iter().map(|(_, file)| lines_of(file)).sum()
}

// The sorted `lines` that fall inside the block's line span.
fn lines_within<'l>(lines: &'l [u32], block: &Block) -> &'l [u32] {
    let start = lines.partition_point(|line| *line < block.start_line);
    let end = lines.partition_point(|line| *line <= block.end_line);
    &lines[start..end.max(start)]
}

pub(super) fn hotspots(
    units: &[PlacedUnit],
    definitions: &BTreeMap<String, Definition<'_>>,
    limit: usize,
) -> Bounded<HealthComplexityRow> {
    let mut ranked: Vec<&PlacedUnit> = units.iter().collect();
    ranked.sort_by(|left, right| {
        let (a, b) = (&left.unit, &right.unit);
        (right.mass().total_cmp(&left.mass()))
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| (a.start_line, a.start_byte).cmp(&(b.start_line, b.start_byte)))
            .then_with(|| a.qualname.cmp(&b.qualname))
    });
    let sites = callable_sites(definitions);
    let rows = ranked
        .iter()
        .map(|placed| hotspot_row(placed, &sites))
        .collect();
    Bounded::cut(rows, limit)
}

// A symbol carries no start byte, so a unit finds its symbol by start line and name.
type Sites<'d> = BTreeMap<(&'d str, u32), Vec<(&'d str, &'d str)>>;

fn callable_sites<'d>(definitions: &'d BTreeMap<String, Definition<'_>>) -> Sites<'d> {
    let mut sites: Sites<'d> = BTreeMap::new();
    for (stable_id, definition) in definitions {
        let symbol = definition.symbol();
        if matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method) {
            let site = sites.entry((definition.path(), symbol.line)).or_default();
            site.push((stable_id.as_str(), symbol.name.as_str()));
        }
    }
    sites
}

fn hotspot_row(placed: &PlacedUnit, sites: &Sites<'_>) -> HealthComplexityRow {
    let unit = &placed.unit;
    HealthComplexityRow {
        path: placed.path.clone(),
        line: unit.start_line,
        label: unit.qualname.clone(),
        cc: unit.cc,
        sloc: unit.sloc,
        mass: placed.mass(),
        stable_id: unit_stable_id(placed, sites),
    }
}

// A symbol's name is the whole qualified name in Lua and only the leaf in Go. Two candidates
// give no id rather than a guess.
fn unit_stable_id(placed: &PlacedUnit, sites: &Sites<'_>) -> Option<String> {
    let qualname = placed.unit.qualname.as_str();
    let candidates = sites.get(&(placed.path.as_str(), placed.unit.start_line))?;
    let found: Vec<&str> = (candidates.iter())
        .filter(|(_, name)| {
            qualname == *name
                || qualname
                    .strip_suffix(*name)
                    .is_some_and(|head| head.ends_with('.'))
        })
        .map(|(stable_id, _)| *stable_id)
        .collect();
    match found.as_slice() {
        [only] => Some((*only).to_owned()),
        _ => None,
    }
}

fn count_f64(count: usize) -> f64 {
    u32::try_from(count).map_or(f64::from(u32::MAX), f64::from)
}

#[cfg(test)]
mod tests;
