//! `contract_drift`: each contract-rule hit that a change adds to a boundary module.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use prompts::messages;
use syntax::Language;

use crate::error::Error;
use crate::finding::{CheckGap, Detail, DriftKey, Finding, PairFinding, Severity};
use crate::finding::{not_checked, sorted};
use crate::globs::in_boundary;
use crate::matching::{Paired, UnitRef, file_units, match_run};
use crate::observed::{HitState, Observed, Outcome};
use crate::pair::{CONTRACT_GAPS, Measured, Pair, Sides, each_pair, read_sides};
use crate::rules::{CompiledRules, Hit, compile, hits, rules_for};

const MODULE_UNIT: &str = "<module>";

type HitKey<'a> = (&'static str, &'a str, &'a str, String);

/// Report each contract-rule hit that a change adds to a file that one of `globs` matches.
///
/// # Errors
///
/// Returns an error when two admitted pairs name one path, when a side fails in a way that is
/// not a parse or tokenizer failure, or when the contract rule query fails on a tree.
pub fn contract_drift(pairs: &[&Pair], globs: &[String]) -> Result<Outcome, Error> {
    if globs.is_empty() {
        return Ok(Outcome {
            findings: no_boundary_rows(pairs),
            observed: Observed::default(),
        });
    }
    let (admitted, mut findings) = admitted(pairs, globs);
    let (measured, gaps) = each_pair(&admitted, |pair, language| {
        read_sides(pair, language, &CONTRACT_GAPS)
    })?;
    findings.extend(gaps);
    let matches = match_run(&file_units(&measured));
    let (found, keys) = new_hits(&measured, &matches)?;
    findings.extend(found);
    let observed = if measured.is_empty() {
        Observed::default()
    } else {
        Observed {
            contract_keys: Some(keys),
            ..Observed::default()
        }
    };
    Ok(Outcome {
        findings: sorted(findings),
        observed,
    })
}

fn no_boundary_rows(pairs: &[&Pair]) -> Vec<PairFinding> {
    if pairs.is_empty() {
        return Vec::new();
    }
    let message = messages::CONTRACT_NO_BOUNDARY.render(io::CONFIG_FILENAME);
    let evidence = messages::CONTRACT_NO_BOUNDARY_EVIDENCE;
    let gap = Some(CheckGap::NoBoundaryGlobs);
    vec![not_checked(
        crate::CONTRACT_DRIFT,
        "",
        message,
        evidence,
        gap,
    )]
}

fn admitted<'a>(pairs: &[&'a Pair], globs: &[String]) -> (Vec<&'a Pair>, Vec<PairFinding>) {
    let mut admitted = Vec::new();
    let mut rows = Vec::new();
    for pair in pairs {
        let Some(language) = pair.language() else {
            continue;
        };
        if !in_boundary(pair.path(), globs) {
            continue;
        }
        if rules_for(language).next().is_some() {
            admitted.push(*pair);
        } else if pair.has_new() {
            rows.push(no_rules_row(pair.path(), language));
        }
    }
    (admitted, rows)
}

fn no_rules_row(path: &str, language: Language) -> PairFinding {
    let message = messages::CONTRACT_NO_RULES.render((path, language.name()));
    let gap = Some(CheckGap::NoRulesForLanguage(language));
    not_checked(crate::CONTRACT_DRIFT, path, message, language.name(), gap)
}

// A new unit that the matching paired keeps its old identity, so a rename or a move to
// another file of the run moves no hit.
fn new_hits(
    measured: &[Sides<'_>],
    matches: &[Paired<'_>],
) -> Result<(Vec<PairFinding>, Vec<HitState>), Error> {
    let rules = compiled(measured);
    let identity: HashMap<(usize, u32), UnitRef<'_>> = matches
        .iter()
        .map(|paired| {
            let new = paired.new;
            ((new.file, new.unit.start_byte), paired.old.unwrap_or(new))
        })
        .collect();
    let mut carried = carried(measured, &rules)?;
    let mut fresh = Vec::new();
    let mut states = Vec::new();
    for (file, sides) in measured.iter().enumerate() {
        let Some(new) = sides.new else {
            continue;
        };
        let path = sides.pair.path();
        let new_units = &new.units;
        for hit in side_hits(sides, new, &rules)? {
            let own = hit.unit.map(|index| &new_units[index]);
            let owner = own.map(|unit| identity[&(file, unit.start_byte)]);
            let rule = hit.rule.id;
            states.push(HitState {
                path: path.to_owned(),
                rule: rule.to_owned(),
                unit: own
                    .map_or(MODULE_UNIT, |unit| unit.qualname.as_str())
                    .to_owned(),
                capture: hit.capture.clone(),
            });
            let key = hit_key(&hit, owner.map(|unit| (unit.path, qualname(&unit))), path);
            match carried.get_mut(&key) {
                Some(count) if *count > 0 => *count -= 1,
                _ => fresh.push(hit_finding(path, &hit, qualname_or_module(owner))),
            }
        }
    }
    states.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((fresh, states))
}

fn compiled(measured: &[Sides<'_>]) -> HashMap<Language, CompiledRules> {
    let mut rules = HashMap::new();
    for sides in measured {
        if let Entry::Vacant(slot) = rules.entry(sides.language) {
            slot.insert(compile(sides.language));
        }
    }
    rules
}

fn side_hits<'r>(
    sides: &Sides<'_>,
    side: &Measured,
    rules: &'r HashMap<Language, CompiledRules>,
) -> Result<Vec<Hit<'r>>, Error> {
    let Some(compiled) = rules.get(&sides.language) else {
        return Ok(Vec::new());
    };
    hits(side.parsed(), sides.pair.path(), &side.units, compiled)
}

fn carried<'a>(
    measured: &[Sides<'a>],
    rules: &HashMap<Language, CompiledRules>,
) -> Result<HashMap<HitKey<'a>, usize>, Error> {
    let mut carried = HashMap::new();
    for sides in measured {
        let Some(old) = sides.old else {
            continue;
        };
        let path = sides.pair.path();
        let old_units = &old.units;
        for hit in side_hits(sides, old, rules)? {
            let owner = hit
                .unit
                .map(|index| (path, old_units[index].qualname.as_str()));
            *carried.entry(hit_key(&hit, owner, path)).or_insert(0) += 1;
        }
    }
    Ok(carried)
}

fn hit_key<'a>(hit: &Hit<'_>, owner: Option<(&'a str, &'a str)>, path: &'a str) -> HitKey<'a> {
    let (place, unit) = owner.unwrap_or((path, MODULE_UNIT));
    (hit.rule.id, place, unit, hit.capture.clone())
}

fn qualname<'a>(unit: &UnitRef<'a>) -> &'a str {
    unit.unit.qualname.as_str()
}

fn qualname_or_module(owner: Option<UnitRef<'_>>) -> String {
    owner.map_or(MODULE_UNIT, |unit| qualname(&unit)).to_owned()
}

fn hit_finding(path: &str, hit: &Hit<'_>, unit: String) -> PairFinding {
    let rule = hit.rule.id;
    let capture = hit.capture.as_str();
    let message = messages::CONTRACT_NEW_HIT.render((rule, &unit, capture, hit.rule.message));
    let evidence = messages::CONTRACT_EVIDENCE.render((rule, &unit, capture));
    let severity = match hit.rule.severity {
        prompts::Severity::Warning => Severity::Warning,
        prompts::Severity::Advisory => Severity::Advisory,
    };
    let finding = Finding {
        check: crate::CONTRACT_DRIFT,
        severity,
        message,
        path: path.to_owned(),
        line: hit.line,
        evidence,
    };
    let key = DriftKey {
        rule: rule.to_owned(),
        unit,
        capture: hit.capture.clone(),
    };
    PairFinding {
        finding,
        detail: Some(Detail::Drift(key)),
    }
}
