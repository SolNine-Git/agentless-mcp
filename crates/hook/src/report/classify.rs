//! Each logged finding's class: not applied, unresolved, unobserved, acted on, or not acted on.

use std::collections::{BTreeSet, HashMap};

use ::text::CodePointString;
use io::GatedCheck;

use crate::record::Outcome;
use crate::report::field::Int;
use crate::report::log::{Finding, Observed, Record, Tracked, UnitCc};
use crate::report::transcript::{Status, Transcripts};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    NotApplied,
    Unresolved,
    Unobserved,
    ActedOn,
    NotActedOn,
}

impl Class {
    // Every class, in the order the report counts them.
    pub(crate) const ALL: [Class; 5] = [
        Self::NotApplied,
        Self::Unresolved,
        Self::Unobserved,
        Self::ActedOn,
        Self::NotActedOn,
    ];
    // The classes of a finding whose call was applied.
    pub(crate) const APPLIED: [Class; 3] = [Self::Unobserved, Self::ActedOn, Self::NotActedOn];

    pub(crate) fn name(self) -> &'static str {
        [
            "not applied",
            "unresolved",
            "unobserved",
            "acted on",
            "not acted on",
        ][self as usize]
    }

    pub(crate) fn applied(self) -> bool {
        Self::APPLIED.contains(&self)
    }
}

// The checks of the findings: the gated checks in their order, then the others in sorted order.
pub(crate) fn checks(classified: &[Classified<'_>]) -> Vec<CodePointString> {
    let present: BTreeSet<&CodePointString> =
        classified.iter().map(|item| &item.finding.check).collect();
    let gated = GatedCheck::ALL.map(|check| CodePointString::from(check.name()));
    let (known, other): (Vec<&CodePointString>, Vec<&CodePointString>) =
        present.into_iter().partition(|check| gated.contains(check));
    let known = gated.iter().filter(|check| known.contains(check));
    known.chain(other).cloned().collect()
}

// One finding with its class and a stable id for the labeling file.
pub(crate) struct Classified<'r> {
    pub(crate) finding_id: CodePointString,
    pub(crate) class: Class,
    pub(crate) record: &'r Record,
    pub(crate) finding: &'r Finding,
}

// A call's status: its transcript's result, or unresolved when no result settles it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Resolved {
    Known(Status),
    Unresolved,
}

type Target<'r> = (Option<&'r CodePointString>, Option<&'r CodePointString>);

pub(crate) fn classify<'r>(
    records: &'r [Record],
    transcripts: &Transcripts,
) -> Vec<Classified<'r>> {
    let mut by_target: HashMap<Target<'r>, Vec<&'r Record>> = HashMap::new();
    for record in records.iter().filter(|record| record.observed.is_some()) {
        let target = (record.session_id.as_ref(), record.path.as_ref());
        by_target.entry(target).or_default().push(record);
    }
    let with_findings = records
        .iter()
        .filter(|record| matches!(record.outcome, Outcome::Advisory | Outcome::Blocked));
    with_findings
        .flat_map(|record| classes(record, &by_target, transcripts))
        .collect()
}

// A record with findings always names its call, so its findings get ids.
fn classes<'r>(
    record: &'r Record,
    by_target: &HashMap<Target<'r>, Vec<&'r Record>>,
    transcripts: &Transcripts,
) -> Vec<Classified<'r>> {
    let Some(tool_use_id) = record.tool_use_id.as_ref() else {
        return Vec::new();
    };
    let resolved = status(record, transcripts);
    let target = (record.session_id.as_ref(), record.path.as_ref());
    let later = by_target.get(&target).map_or(&[][..], Vec::as_slice);
    let evidence: Vec<&Observed> = later
        .iter()
        .filter(|later| {
            later.line_no > record.line_no
                && status(later, transcripts) == Resolved::Known(Status::Applied)
        })
        .filter_map(|later| later.observed.as_ref())
        .collect();
    let classified = |(index, finding)| {
        let mut finding_id = tool_use_id.clone();
        finding_id.push_str(&format!("#{index}"));
        Classified {
            finding_id,
            class: class(finding, record.observed.as_ref(), resolved, &evidence),
            record,
            finding,
        }
    };
    record.findings.iter().enumerate().map(classified).collect()
}

fn status(record: &Record, transcripts: &Transcripts) -> Resolved {
    let results = record
        .transcript_path
        .as_ref()
        .and_then(|path| transcripts.get(path))
        .and_then(|transcript| transcript.results.as_ref());
    let found = record
        .tool_use_id
        .as_ref()
        .and_then(|id| results.and_then(|results| results.get(id)));
    found.map_or(Resolved::Unresolved, |status| Resolved::Known(*status))
}

fn class(
    finding: &Finding,
    at_finding: Option<&Observed>,
    status: Resolved,
    evidence: &[&Observed],
) -> Class {
    match status {
        Resolved::Known(Status::Rejected) => return Class::NotApplied,
        Resolved::Unresolved => return Class::Unresolved,
        Resolved::Known(Status::Applied) => {}
    }
    let Some(tracked) = finding.tracked.as_ref() else {
        return Class::Unobserved;
    };
    let seen: Vec<&Observed> = evidence
        .iter()
        .copied()
        .filter(|observed| observes(tracked, observed))
        .collect();
    if seen.is_empty() {
        return Class::Unobserved;
    }
    if seen
        .iter()
        .any(|later| acted(finding, tracked, at_finding, later))
    {
        return Class::ActedOn;
    }
    Class::NotActedOn
}

fn observes(tracked: &Tracked, observed: &Observed) -> bool {
    match tracked {
        Tracked::Complexity { .. } => observed.units_over_threshold.is_some(),
        Tracked::Contract { .. } => observed.contract_keys.is_some(),
        Tracked::Clone { .. } => observed.clone_instances.is_some(),
    }
}

// The finding is undone when the later state no longer holds it, by its check's rule.
fn acted(
    finding: &Finding,
    tracked: &Tracked,
    at_finding: Option<&Observed>,
    later: &Observed,
) -> bool {
    match tracked {
        Tracked::Complexity { cc_old, threshold } => {
            let over = later.units_over_threshold.as_deref();
            over.is_some_and(|over| complexity_acted(finding, cc_old.as_ref(), threshold, over))
        }
        Tracked::Contract { rule, capture } => {
            let Some(after) = later.contract_keys.as_ref() else {
                return false;
            };
            let before = at_finding.and_then(|observed| observed.contract_keys.as_ref());
            fewer(after, before, |hit| {
                hit.path == finding.path && hit.rule == *rule && hit.capture == *capture
            })
        }
        Tracked::Clone { key } => {
            let Some(after) = later.clone_instances.as_ref() else {
                return false;
            };
            let before = at_finding.and_then(|observed| observed.clone_instances.as_ref());
            fewer(after, before, |copy| {
                copy.path == finding.path && copy.key == *key
            })
        }
    }
}

// Undone when fewer rows match than at the finding, or none when that state is unknown.
fn fewer<T>(after: &[T], before: Option<&Vec<T>>, same: impl Fn(&T) -> bool) -> bool {
    let now = after.iter().filter(|row| same(row)).count();
    match before {
        Some(before) => now < before.iter().filter(|row| same(row)).count(),
        None => now == 0,
    }
}

// Growth is undone at the old cc. A crossing or a new unit is undone under the threshold.
fn complexity_acted(
    finding: &Finding,
    cc_old: Option<&Int>,
    threshold: &Int,
    over: &[UnitCc],
) -> bool {
    let limit = match cc_old {
        Some(old) if old > threshold => old,
        _ => threshold,
    };
    !over.iter().any(|row| {
        row.path == finding.path && Some(&row.unit) == finding.unit.as_ref() && row.cc > *limit
    })
}
