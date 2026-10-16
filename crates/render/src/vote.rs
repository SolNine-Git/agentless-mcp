//! The vote report as text: the header, a card per cluster, then the excluded candidates; and its
//! one-line summary.

use patch::{Cluster, VoteReport};

/// Returns the vote report as text: the tier that answered, a card per cluster in rank order,
/// then the candidates excluded before the ladder.
#[must_use]
pub fn vote_text(report: &VoteReport) -> String {
    let reproduction = if report.repro_valid {
        "reproduced the bug on the baseline and counted"
    } else {
        "was not usable and was excluded from the ladder"
    };
    let mut lines = vec![
        format!(
            "# vote over {} candidates -- tier '{}'",
            report.considered,
            report.tier.as_str()
        ),
        format!("# {}", report.tier.detail()),
        format!("# reproduction test {reproduction}"),
        String::new(),
    ];
    if report.clusters.is_empty() {
        lines.push("no candidate survived the ladder; there is nothing to rank.".to_owned());
    }
    lines.extend(report.clusters.iter().flat_map(card));
    if !report.excluded.is_empty() {
        lines.push("excluded before the ladder:".to_owned());
        let excluded = report.excluded.iter();
        lines.extend(excluded.map(|out| format!("  {} -- {}", out.id, out.reason)));
        lines.push(String::new());
    }
    lines.join("\n").trim_end_matches('\n').to_owned() + "\n"
}

// One cluster as an incident card, with the first 16 characters of its key.
fn card(cluster: &Cluster) -> [String; 5] {
    let plural = if cluster.size == 1 {
        "candidate"
    } else {
        "candidates"
    };
    let key: String = cluster.key.chars().take(16).collect();
    [
        format!(
            "rank {}  {} {plural}  key {key}",
            cluster.rank, cluster.size
        ),
        format!(
            "  representative: {} (first appearance #{})",
            cluster.representative, cluster.first_appearance
        ),
        format!("  members: {}", cluster.members.join(", ")),
        format!(
            "  regression passed {}/{}   reproduction passed {}/{}",
            cluster.regression_passed, cluster.size, cluster.reproduction_passed, cluster.size
        ),
        String::new(),
    ]
}

/// Returns the one-line summary of a vote for its receipt: the tier, the survivors, the clusters
/// and the winner.
#[must_use]
pub fn vote_summary(report: &VoteReport) -> String {
    let count = report.clusters.len();
    let plural = if count == 1 { "" } else { "s" };
    let winner = report.winner().filter(|winner| !winner.is_empty());
    format!(
        "tier '{}': {} of {} candidates in {count} cluster{plural}; winner {}",
        report.tier.as_str(),
        report.survived,
        report.considered,
        winner.unwrap_or("none")
    )
}
