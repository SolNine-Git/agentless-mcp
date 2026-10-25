//! The health report's two forms: the three graph sections under one shared header, then the
//! complexity summary and the hotspots in the text, and every section whole in the JSON.

use app::{
    HealthComplexity, HealthComplexityRow, HealthReport, HealthSymbol, Skipped, file_site, locator,
};
use json::fixed;
use kernel::Bounded;
use prompts::messages::{
    HEALTH_COMPLEXITY, HEALTH_COMPLEXITY_WALKED, HEALTH_EXPORTS, HEALTH_EXPORTS_UNLISTED,
    HEALTH_HOTSPOTS, HEALTH_HOTSPOTS_UNLISTED, HEALTH_HUBS, HEALTH_HUBS_UNLISTED, HEALTH_ORPHANS,
    HEALTH_ORPHANS_UNLISTED, HEALTH_OVER, HEALTH_OVER_EXCLUDED, HEALTH_UNMEASURED,
    HEALTH_UNMEASURED_UNLISTED,
};
use prompts::{Count, Template};
use serde::Serialize;
use text::one_line;

use crate::envelope::wrap_json_whole;
use crate::{Rendered, View};

// What one section prints: its header, its cut line, and its text when it found nothing.
struct Wording {
    header: &'static Template<Count>,
    unlisted: &'static Template<(Count, Count)>,
    empty: &'static str,
}

const ORPHANS: Wording = Wording {
    header: &HEALTH_ORPHANS,
    unlisted: &HEALTH_ORPHANS_UNLISTED,
    empty: "no orphan candidates: every function has a counted edge",
};

const UNUSED: Wording = Wording {
    header: &HEALTH_EXPORTS,
    unlisted: &HEALTH_EXPORTS_UNLISTED,
    empty: "no unused exports: every public function outside a function body is reached",
};

// The heading counts every symbol with an edge, so it names symbols; the rows it cut are hubs.
const HUBS: Wording = Wording {
    header: &HEALTH_HUBS,
    unlisted: &HEALTH_HUBS_UNLISTED,
    empty: "no hubs: no function, method or class carries a counted edge",
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
    let header = if report.excluded > 0 {
        HEALTH_OVER_EXCLUDED.render((report.symbols, report.excluded))
    } else {
        HEALTH_OVER.render(report.symbols)
    };
    let mut lines = vec![
        header,
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

    fn line(&self, unlisted: &Template<(Count, Count)>) -> Option<String> {
        (self.omitted > 0).then(|| unlisted.render((self.omitted, self.limit)))
    }
}

// Keyed on the count before the cut: a limit must never read as a clean repository.
fn section_lines(rows: &[HealthSymbol], cut: &Cut, wording: &Wording) -> Vec<String> {
    if cut.total == 0 {
        return vec![String::new(), wording.empty.to_owned()];
    }
    let mut lines = vec![String::new(), wording.header.render(cut.total)];
    lines.extend(rows.iter().map(symbol_row));
    lines.extend(cut.line(wording.unlisted));
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
    let header = HEALTH_COMPLEXITY.render((
        summary.units,
        summary.measured(),
        summary.threshold as usize,
        summary.over_threshold,
        &fixed(summary.erosion, 3),
        &fixed(summary.clone_share(), 3),
    ));
    let mut lines = vec![String::new(), header];
    if summary.stopped_by_budget() {
        lines.push(HEALTH_COMPLEXITY_WALKED.render((summary.walked, summary.files)));
    }
    let listing = &summary.not_measured;
    lines.extend(unmeasured_lines(listing.rows(), &Cut::of(listing)));
    lines
}

fn unmeasured_lines(rows: &[Skipped], cut: &Cut) -> Vec<String> {
    if cut.total == 0 {
        return Vec::new();
    }
    let mut lines = vec![HEALTH_UNMEASURED.render(cut.total)];
    lines.extend(
        (rows.iter())
            .map(|entry| format!("  {}  {}", one_line(&entry.path), one_line(&entry.reason))),
    );
    lines.extend(cut.line(&HEALTH_UNMEASURED_UNLISTED));
    lines
}

fn hotspot_lines(rows: &[HealthComplexityRow], cut: &Cut) -> Vec<String> {
    if cut.total == 0 {
        return vec![
            String::new(),
            "no hotspots: no unit was measured".to_owned(),
        ];
    }
    let mut lines = vec![String::new(), HEALTH_HOTSPOTS.render(cut.total)];
    lines.extend(rows.iter().map(hotspot_row));
    lines.extend(cut.line(&HEALTH_HOTSPOTS_UNLISTED));
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
