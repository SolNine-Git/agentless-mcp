//! The lint report: per candidate, the findings in arrival order under a count of each severity.

use app::{Finding, LintCandidate, LintReport, PairFinding, Severity};

use crate::envelope::Listing;
use text::one_line;

impl Listing for LintReport {
    fn items(&self) -> usize {
        self.candidates.len()
    }

    fn prefix(&self, kept: usize) -> Self {
        let mut prefix = self.clone();
        prefix.candidates.truncate(kept);
        prefix
    }
}

pub(crate) fn render_lint(report: &LintReport) -> String {
    if report.candidates.is_empty() {
        return "no candidates to lint\n".to_owned();
    }
    let blocks: Vec<String> = report.candidates.iter().map(candidate_block).collect();
    blocks.join("\n\n") + "\n"
}

fn candidate_block(candidate: &LintCandidate) -> String {
    let counts = severity_counts(&candidate.findings);
    let mut lines = vec![format!("{}: {counts}", one_line(&candidate.id))];
    lines.extend(
        candidate
            .findings
            .iter()
            .map(|found| finding_lines(&found.finding)),
    );
    lines.join("\n")
}

fn finding_lines(finding: &Finding) -> String {
    format!(
        "  [{}] {}  {}\n      {}\n      evidence: {}",
        finding.severity.as_str(),
        one_line(finding.check),
        one_line(&finding.location()),
        one_line(&finding.message),
        one_line(&finding.evidence),
    )
}

// "1 warning, 2 advisory": each severity once, most urgent first, with no plural.
fn severity_counts(findings: &[PairFinding]) -> String {
    let mut tally: Vec<(Severity, usize)> = Vec::new();
    for found in findings {
        let severity = found.finding.severity;
        match tally.iter_mut().find(|(seen, _)| *seen == severity) {
            Some((_, count)) => *count += 1,
            None => tally.push((severity, 1)),
        }
    }
    if tally.is_empty() {
        return "no findings".to_owned();
    }
    tally.sort_by_key(|&(severity, _)| urgency(severity));
    let counts: Vec<String> = tally
        .iter()
        .map(|(severity, count)| format!("{count} {}", severity.as_str()))
        .collect();
    counts.join(", ")
}

const fn urgency(severity: Severity) -> u8 {
    match severity {
        Severity::Warning => 0,
        Severity::Advisory => 1,
        Severity::NotChecked => 2,
    }
}
