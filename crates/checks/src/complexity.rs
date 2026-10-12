//! `complexity_delta`: each unit that a change grows past the threshold, takes over it, or adds.

use prompts::messages;

use crate::error::Error;
use crate::finding::{Detail, Finding, Match, PairFinding, Severity, UnitDelta, sorted};
use crate::matching::{Paired, file_units, match_run};
use crate::observed::{Observed, Outcome, UnitChange, UnitState};
use crate::pair::{COMPLEXITY_GAPS, Pair, Sides, each_pair, metrics, read_sides};

/// Report each unit that a change grows above `threshold`, takes over it, or adds over it.
///
/// # Errors
///
/// Returns an error when two pairs name one path, or when a side fails in a way that is not a
/// parse or tokenizer failure.
pub fn complexity_delta(pairs: &[&Pair], threshold: u32) -> Result<Outcome, Error> {
    let (measured, mut findings) = each_pair(pairs, |pair, language| {
        read_sides(pair, language, &COMPLEXITY_GAPS)
    })?;
    let matches = match_run(&file_units(&measured));
    findings.extend(
        matches
            .iter()
            .filter_map(|paired| unit_finding(paired, threshold)),
    );
    let observed = if measured.is_empty() {
        Observed::default()
    } else {
        Observed {
            units_over_threshold: Some(over_threshold(&measured, threshold)),
            units_changed: Some(changed(&matches)),
            ..Observed::default()
        }
    };
    Ok(Outcome {
        findings: sorted(findings),
        observed,
    })
}

enum Rise {
    New,
    Grew,
    Crossed,
}

fn rise(paired: &Paired<'_>, threshold: u32) -> Option<Rise> {
    let new = paired.new.unit.cc;
    match paired.old.map(|old| old.unit.cc) {
        None if new > threshold => Some(Rise::New),
        Some(old) if old > threshold && new > old => Some(Rise::Grew),
        Some(old) if old <= threshold && threshold < new => Some(Rise::Crossed),
        None | Some(_) => None,
    }
}

fn unit_finding(paired: &Paired<'_>, threshold: u32) -> Option<PairFinding> {
    let rise = rise(paired, threshold)?;
    let new = paired.new.unit;
    let qualname = new.qualname.as_str();
    let (cc, sloc, limit) = (new.cc as usize, new.sloc as usize, threshold as usize);
    let (old_cc, old_sloc) = paired
        .old
        .map_or((0, 0), |old| (old.unit.cc as usize, old.unit.sloc as usize));
    let (message, evidence) = match rise {
        Rise::New => (
            messages::COMPLEXITY_NEW_UNIT.render((qualname, cc, sloc, limit)),
            messages::COMPLEXITY_NEW_UNIT_EVIDENCE.render((cc, sloc, limit)),
        ),
        Rise::Grew => (
            messages::COMPLEXITY_GREW.render((qualname, old_cc, cc, old_sloc, sloc, limit)),
            messages::COMPLEXITY_EVIDENCE.render((old_cc, cc, old_sloc, sloc, limit)),
        ),
        Rise::Crossed => (
            messages::COMPLEXITY_CROSSED.render((qualname, old_cc, cc, old_sloc, sloc, limit)),
            messages::COMPLEXITY_EVIDENCE.render((old_cc, cc, old_sloc, sloc, limit)),
        ),
    };
    let finding = Finding {
        check: crate::COMPLEXITY_DELTA,
        severity: Severity::Warning,
        message,
        path: paired.new.path.to_owned(),
        line: new.start_line,
        evidence,
    };
    Some(PairFinding {
        finding,
        detail: Some(Detail::Unit(delta(paired, threshold))),
    })
}

fn delta(paired: &Paired<'_>, threshold: u32) -> UnitDelta {
    let new = paired.new.unit;
    UnitDelta {
        unit: new.qualname.clone(),
        via: paired.via,
        cc_old: paired.old.map(|old| old.unit.cc),
        cc_new: new.cc,
        sloc_old: paired.old.map(|old| old.unit.sloc),
        sloc_new: new.sloc,
        threshold,
    }
}

fn over_threshold(measured: &[Sides<'_>], threshold: u32) -> Vec<UnitState> {
    let mut rows: Vec<UnitState> = measured
        .iter()
        .flat_map(|sides| {
            metrics(sides.new)
                .units
                .iter()
                .map(move |unit| (sides, unit))
        })
        .filter(|(_, unit)| unit.cc > threshold)
        .map(|(sides, unit)| UnitState {
            path: sides.pair.path().to_owned(),
            unit: unit.qualname.clone(),
            line: unit.start_line,
            cc: unit.cc,
            sloc: unit.sloc,
        })
        .collect();
    rows.sort_by(|a, b| (&a.path, a.line, &a.unit).cmp(&(&b.path, b.line, &b.unit)));
    rows
}

fn changed(matches: &[Paired<'_>]) -> Vec<UnitChange> {
    let mut rows: Vec<UnitChange> = matches
        .iter()
        .filter(|paired| paired.via != Match::Body)
        .map(|paired| UnitChange {
            path: paired.new.path.to_owned(),
            unit: paired.new.unit.qualname.clone(),
            line: paired.new.unit.start_line,
            via: paired.via,
            cc_old: paired.old.map(|old| old.unit.cc),
            cc_new: paired.new.unit.cc,
            sloc_old: paired.old.map(|old| old.unit.sloc),
            sloc_new: paired.new.unit.sloc,
        })
        .collect();
    rows.sort_by(|a, b| (&a.path, a.line, &a.unit).cmp(&(&b.path, b.line, &b.unit)));
    rows
}
