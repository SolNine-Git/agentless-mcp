//! The seeded labeling sample: up to 30 applied findings per check, as rows of a new CSV file.

use crate::report::classify::{Classified, checks};
use crate::report::csv::excel_row;
use crate::report::random::Twister;
use crate::text::lossy;

pub(crate) const SAMPLE_SIZE: usize = 30;

// The labeling file's columns. The report reads the first three back.
pub(crate) const FINDING_ID: &str = "finding_id";
pub(crate) const CHECK: &str = "check";
pub(crate) const LABEL: &str = "label";
const FIELDS: [&str; 9] = [
    FINDING_ID,
    CHECK,
    LABEL,
    "session_id",
    "path",
    "line",
    "unit",
    "message",
    "evidence",
];

// The same log and seed draw the same rows: the generator of each check is seeded "<seed>:<check>".
pub(crate) fn sample<'c, 'r>(
    classified: &'c [Classified<'r>],
    seed: &str,
) -> Vec<&'c Classified<'r>> {
    let mut drawn = Vec::new();
    for check in checks(classified) {
        let population: Vec<&Classified<'r>> = classified
            .iter()
            .filter(|item| item.finding.check == check && item.class.applied())
            .collect();
        let size = population.len().min(SAMPLE_SIZE);
        let mut twister = Twister::from_text(&format!("{seed}:{}", lossy(&check)));
        let picks = twister.sample(population.len(), size);
        drawn.extend(picks.into_iter().map(|index| population[index]));
    }
    drawn
}

// The labeling file: the header, then one row per drawn finding with an empty label.
pub(crate) fn sample_csv(rows: &[&Classified<'_>]) -> String {
    let mut text = excel_row(&FIELDS.map(str::to_owned));
    for item in rows {
        let finding = item.finding;
        let unit = finding.unit.as_ref().map_or_else(String::new, lossy);
        let session = item
            .record
            .session_id
            .as_ref()
            .map_or_else(String::new, lossy);
        text.push_str(&excel_row(&[
            lossy(&item.finding_id),
            lossy(&finding.check),
            String::new(),
            session,
            lossy(&finding.path),
            finding.line.to_string(),
            unit,
            lossy(&finding.message),
            lossy(&finding.evidence),
        ]));
    }
    text
}
