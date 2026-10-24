//! The health report's two forms: the three graph sections under one shared header, then the
//! complexity summary and the hotspots in the text, and every section whole in the JSON.

use app::{
    HealthComplexity, HealthComplexityRow, HealthReport, HealthSymbol, Skipped, file_site, locator,
    omitted_line,
};
use json::fixed;
use kernel::Bounded;
use serde::Serialize;
use text::one_line;

use crate::envelope::wrap_json_whole;
use crate::{Rendered, View};

// What one section calls itself, whether or not it found anything. `omitted` is the noun of the
// cut line when it differs from `plural`.
struct Wording {
    singular: &'static str,
    plural: &'static str,
    criterion: &'static str,
    empty: &'static str,
    omitted: &'static str,
}

const ORPHANS: Wording = Wording {
    singular: "orphan candidate",
    plural: "orphan candidates",
    criterion: "function, no counted edge in or out",
    empty: "no orphan candidates: every function has a counted edge",
    omitted: "orphan candidates",
};

const UNUSED: Wording = Wording {
    singular: "unused export",
    plural: "unused exports",
    criterion: "public function, no counted edge in",
    empty: "no unused exports: every public function is reached",
    omitted: "unused exports",
};

// The heading counts every symbol with an edge, so it names symbols; the rows it cut are hubs.
const HUBS: Wording = Wording {
    singular: "symbol carries a counted edge",
    plural: "symbols carry a counted edge",
    criterion: "function, method or class, highest counted degree first",
    empty: "no hubs: no function, method or class carries a counted edge",
    omitted: "hubs",
};

impl View for HealthReport {
    fn rendered(&self) -> Rendered<'_> {
        Rendered::new(
            health_text(self),
            HealthJson::of(self),
            None,
            wrap_json_whole,
        )
    }
}

fn health_text(report: &HealthReport) -> String {
    let excluded = if report.excluded > 0 {
        format!("; {} excluded as test or fixture paths", report.excluded)
    } else {
        String::new()
    };
    let mut lines = vec![
        format!(
            "health over {} {}{excluded}",
            report.symbols,
            plural(report.symbols, "symbol")
        ),
        "degree counts same-file and resolved-via-import edges only; unique and \
         name-only-ambiguous matches are discounted and named per row"
            .to_owned(),
        "methods are ranked as hubs and never reported as orphans: a call through a selector \
         resolves to no edge, so every method would be a permanent candidate"
            .to_owned(),
    ];
    for (section, wording) in [
        (&report.orphans, &ORPHANS),
        (&report.unused_exports, &UNUSED),
        (&report.hubs, &HUBS),
    ] {
        lines.extend(section_lines(section.rows(), &Cut::of(section), wording));
    }
    lines.extend(complexity_lines(&report.complexity));
    let hotspots = &report.hotspots;
    lines.extend(hotspot_lines(hotspots.rows(), &Cut::of(hotspots)));
    lines.join("\n") + "\n"
}

// A listing's count before the cut, the rows it left out, and the limit that cut it.
struct Cut {
    total: usize,
    omitted: usize,
    limit: usize,
}

impl Cut {
    fn of<R>(listing: &Bounded<R>) -> Cut {
        Cut {
            total: listing.total(),
            omitted: listing.omitted(),
            limit: listing.limit(),
        }
    }

    fn line(&self, noun: &str) -> Option<String> {
        (self.omitted > 0).then(|| omitted_line(self.omitted, noun, self.limit))
    }
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        noun.to_owned()
    } else {
        format!("{noun}s")
    }
}

// Keyed on the count before the cut: a limit must never read as a clean repository.
fn section_lines(rows: &[HealthSymbol], cut: &Cut, wording: &Wording) -> Vec<String> {
    if cut.total == 0 {
        return vec![String::new(), wording.empty.to_owned()];
    }
    let noun = if cut.total == 1 {
        wording.singular
    } else {
        wording.plural
    };
    let mut lines = vec![
        String::new(),
        format!("{} {noun} ({})", cut.total, wording.criterion),
    ];
    lines.extend(rows.iter().map(symbol_row));
    lines.extend(cut.line(wording.omitted));
    lines
}

fn symbol_row(row: &HealthSymbol) -> String {
    let discounted: Vec<String> = (row.discounted.iter())
        .map(|entry| format!("{} {}", entry.count, one_line(entry.tier)))
        .collect();
    let note = if discounted.is_empty() {
        String::new()
    } else {
        format!("  -- discounted: {}", discounted.join(", "))
    };
    let line = row.line.to_string();
    format!(
        "  {}  {}  {}  in {}  out {}{note}",
        locator(&row.stable_id, "", Some(&line)),
        one_line(&row.label),
        one_line(row.kind),
        row.in_degree,
        row.out_degree
    )
}

fn complexity_lines(summary: &HealthComplexity) -> Vec<String> {
    if summary.files == 0 {
        let none = "complexity: no tier-1 source files to measure outside test and fixture paths";
        return vec![String::new(), none.to_owned()];
    }
    let measured = summary.measured();
    let mut lines = vec![
        String::new(),
        format!(
            "complexity over {} {} in {measured} {} (threshold cc > {}): {} over the \
             threshold, erosion {}, clone share {}",
            summary.units,
            plural(summary.units, "unit"),
            plural(measured, "file"),
            summary.threshold,
            summary.over_threshold,
            fixed(summary.erosion, 3),
            fixed(summary.clone_share(), 3)
        ),
    ];
    if summary.stopped_by_budget() {
        lines.push(format!(
            "complexity: {} of {} files walked before the time budget",
            summary.walked, summary.files
        ));
    }
    let listing = &summary.not_measured;
    lines.extend(unmeasured_lines(listing.rows(), &Cut::of(listing)));
    lines
}

fn unmeasured_lines(rows: &[Skipped], cut: &Cut) -> Vec<String> {
    if cut.total == 0 {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "complexity: {} {} not measured",
        cut.total,
        plural(cut.total, "file")
    )];
    lines.extend(
        (rows.iter())
            .map(|entry| format!("  {}  {}", one_line(&entry.path), one_line(&entry.reason))),
    );
    lines.extend(cut.line("unmeasured files"));
    lines
}

fn hotspot_lines(rows: &[HealthComplexityRow], cut: &Cut) -> Vec<String> {
    if cut.total == 0 {
        return vec![
            String::new(),
            "no hotspots: no unit was measured".to_owned(),
        ];
    }
    let mut lines = vec![
        String::new(),
        format!(
            "{} {} ranked by mass (named functions and methods, mass = cc * sqrt(sloc), highest \
             first)",
            cut.total,
            plural(cut.total, "unit")
        ),
    ];
    lines.extend(rows.iter().map(hotspot_row));
    lines.extend(cut.line("hotspots"));
    lines
}

fn hotspot_row(row: &HealthComplexityRow) -> String {
    let (site, note) = match &row.stable_id {
        Some(stable_id) => (locator(stable_id, "", Some(&row.line.to_string())), ""),
        None => (
            file_site(&row.path, row.line),
            "  -- metric-only unit, no stable id",
        ),
    };
    format!(
        "  {site}  {}  cc {}  sloc {}  mass {}{note}",
        one_line(&row.label),
        row.cc,
        row.sloc,
        fixed(row.mass, 1)
    )
}

#[derive(Serialize)]
struct HealthJson<'a> {
    symbols: usize,
    excluded: usize,
    orphans: ListingJson<SymbolJson<'a>>,
    unused_exports: ListingJson<SymbolJson<'a>>,
    hubs: ListingJson<SymbolJson<'a>>,
    complexity: ComplexityJson<'a>,
    hotspots: ListingJson<HotspotJson<'a>>,
}

#[derive(Serialize)]
struct ListingJson<T> {
    total: usize,
    limit: usize,
    omitted: usize,
    rows: Vec<T>,
}

impl<T> ListingJson<T> {
    fn of<'r, R: 'r>(listing: &'r Bounded<R>, row: impl Fn(&'r R) -> T) -> Self {
        ListingJson {
            total: listing.total(),
            limit: listing.limit(),
            omitted: listing.omitted(),
            rows: listing.rows().iter().map(row).collect(),
        }
    }
}

#[derive(Serialize)]
struct SymbolJson<'a> {
    stable_id: &'a str,
    path: &'a str,
    line: u32,
    label: &'a str,
    kind: &'a str,
    in_degree: usize,
    out_degree: usize,
    degree: usize,
    discounted: Vec<DiscountedJson<'a>>,
}

#[derive(Serialize)]
struct DiscountedJson<'a> {
    tier: &'a str,
    count: usize,
}

#[derive(Serialize)]
struct ComplexityJson<'a> {
    threshold: u32,
    files: usize,
    walked: usize,
    measured: usize,
    stopped_by_budget: bool,
    units: usize,
    over_threshold: usize,
    erosion: f64,
    sloc: usize,
    clone_lines: usize,
    clone_share: f64,
    not_measured: ListingJson<UnmeasuredJson<'a>>,
}

#[derive(Serialize)]
struct UnmeasuredJson<'a> {
    path: &'a str,
    reason: &'a str,
}

#[derive(Serialize)]
struct HotspotJson<'a> {
    stable_id: Option<&'a str>,
    metric_only: bool,
    path: &'a str,
    line: u32,
    label: &'a str,
    cc: u32,
    sloc: u32,
    mass: f64,
}

impl<'a> HealthJson<'a> {
    fn of(report: &'a HealthReport) -> Self {
        HealthJson {
            symbols: report.symbols,
            excluded: report.excluded,
            orphans: ListingJson::of(&report.orphans, symbol_json),
            unused_exports: ListingJson::of(&report.unused_exports, symbol_json),
            hubs: ListingJson::of(&report.hubs, symbol_json),
            complexity: complexity_json(&report.complexity),
            hotspots: ListingJson::of(&report.hotspots, hotspot_json),
        }
    }
}

fn symbol_json(row: &HealthSymbol) -> SymbolJson<'_> {
    let discounted = row.discounted.iter().map(|entry| DiscountedJson {
        tier: entry.tier,
        count: entry.count,
    });
    SymbolJson {
        stable_id: &row.stable_id,
        path: &row.path,
        line: row.line,
        label: &row.label,
        kind: row.kind,
        in_degree: row.in_degree,
        out_degree: row.out_degree,
        degree: row.in_degree + row.out_degree,
        discounted: discounted.collect(),
    }
}

fn complexity_json(summary: &HealthComplexity) -> ComplexityJson<'_> {
    ComplexityJson {
        threshold: summary.threshold,
        files: summary.files,
        walked: summary.walked,
        measured: summary.measured(),
        stopped_by_budget: summary.stopped_by_budget(),
        units: summary.units,
        over_threshold: summary.over_threshold,
        erosion: summary.erosion,
        sloc: summary.sloc,
        clone_lines: summary.clone_lines,
        clone_share: summary.clone_share(),
        not_measured: ListingJson::of(&summary.not_measured, unmeasured_json),
    }
}

fn unmeasured_json(entry: &Skipped) -> UnmeasuredJson<'_> {
    UnmeasuredJson {
        path: &entry.path,
        reason: &entry.reason,
    }
}

fn hotspot_json(row: &HealthComplexityRow) -> HotspotJson<'_> {
    HotspotJson {
        stable_id: row.stable_id.as_deref(),
        metric_only: row.stable_id.is_none(),
        path: &row.path,
        line: row.line,
        label: &row.label,
        cc: row.cc,
        sloc: row.sloc,
        mass: row.mass,
    }
}
