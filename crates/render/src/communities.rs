//! The communities' two forms: each community with its members and the partition's scores in
//! the text, and every community with its weights in the JSON, trimmed at the communities.

use std::cmp::Ordering;

use app::{CommunityReport, CommunityRow};
use json::{fixed, general, round_half_even};
use prompts::messages::{
    COMMUNITIES_HEADER, COMMUNITIES_NOT_CONVERGED, COMMUNITIES_NOTHING_PARSED,
    COMMUNITIES_UNLISTED, COMMUNITIES_WEAK, COMMUNITY_FILES_UNLISTED, GROUPED_FILES,
};
use serde::{Serialize, Serializer};
use text::one_line;

use crate::envelope::{Listing, wrap_json};
use crate::{Rendered, View};

// The published reading of modularity holds at resolution 1 alone, so only the standard score
// is ever compared with this.
const WEAK_MODULARITY_THRESHOLD: f64 = 0.3;

impl View for CommunityReport {
    fn rendered(&self) -> Rendered<'_> {
        Rendered::new(
            communities_text(self),
            CommunitiesJson::of(self),
            None,
            wrap_json,
        )
    }
}

// Keyed on the total, so a limit never reads as an empty repository.
fn communities_text(report: &CommunityReport) -> String {
    let total = report.communities.total();
    if total == 0 {
        return format!("{COMMUNITIES_NOTHING_PARSED}\n");
    }
    let scaled = flat_zero(round_half_even(report.modularity, 3));
    let mut lines = vec![COMMUNITIES_HEADER.render((
        total,
        report.files,
        &fixed(scaled, 3),
        &general(report.resolution),
    ))];
    lines.extend(partition_notes(report, scaled));
    for (at, community) in report.communities.rows().iter().enumerate() {
        lines.extend(community_lines(at + 1, community));
    }
    let omitted = report.communities.omitted();
    if omitted > 0 {
        lines.push(COMMUNITIES_UNLISTED.render((omitted, report.communities.limit())));
    }
    lines.join("\n") + "\n"
}

fn community_lines(number: usize, community: &CommunityRow) -> Vec<String> {
    let size = GROUPED_FILES.render(community.members.total());
    let label = one_line(&community.label);
    let mut lines = vec![format!("  {number:>3}. {label}  {size}")];
    let members = community.members.rows().iter();
    lines.extend(members.map(|member| format!("       {}", one_line(member))));
    let omitted = community.members.omitted();
    if omitted > 0 {
        let limit = community.members.limit();
        lines.push(COMMUNITY_FILES_UNLISTED.render((omitted, limit)));
    }
    lines
}

fn partition_notes(report: &CommunityReport, scaled: f64) -> Vec<String> {
    let mut notes = Vec::new();
    let standard = flat_zero(round_half_even(report.standard_modularity, 3));
    // Named only when the knob moved it: the weak-partition note is keyed on this number.
    if standard.partial_cmp(&scaled) != Some(Ordering::Equal) {
        notes.push(format!(
            "  modularity at resolution 1: {}; compare this number against 0.3, not the one above",
            fixed(standard, 3)
        ));
    }
    if !report.converged {
        notes.push(COMMUNITIES_NOT_CONVERGED.to_owned());
    }
    if weak_partition(report) {
        notes.push(COMMUNITIES_WEAK.to_owned());
    }
    notes
}

fn weak_partition(report: &CommunityReport) -> bool {
    report.standard_modularity < WEAK_MODULARITY_THRESHOLD
}

// A zero that a summation left negative is accumulation noise, and it prints as zero.
fn flat_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

#[derive(Clone, Serialize)]
struct CommunitiesJson<'a> {
    total: usize,
    limit: usize,
    omitted: usize,
    modularity: f64,
    standard_modularity: f64,
    weak_partition: bool,
    resolution: f64,
    files: usize,
    converged: bool,
    communities: Vec<CommunityJson<'a>>,
}

#[derive(Clone, Serialize)]
struct CommunityJson<'a> {
    label: &'a str,
    size: usize,
    members: &'a [String],
    omitted: usize,
    internal_weight: Weight,
    total_weight: Weight,
}

// A weight rounded to six places. A community with no edge sums nothing, which prints as the
// integer 0.
#[derive(Clone, Copy)]
struct Weight(f64);

impl Serialize for Weight {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.0 == 0.0 {
            return serializer.serialize_u64(0);
        }
        serializer.serialize_f64(round_half_even(self.0, 6))
    }
}

impl<'a> CommunitiesJson<'a> {
    fn of(report: &'a CommunityReport) -> Self {
        let communities = report
            .communities
            .rows()
            .iter()
            .map(|community| CommunityJson {
                label: &community.label,
                size: community.members.total(),
                members: community.members.rows(),
                omitted: community.members.omitted(),
                internal_weight: Weight(community.internal_weight),
                total_weight: Weight(community.total_weight),
            });
        CommunitiesJson {
            total: report.communities.total(),
            limit: report.communities.limit(),
            omitted: report.communities.omitted(),
            modularity: flat_zero(round_half_even(report.modularity, 6)),
            standard_modularity: flat_zero(round_half_even(report.standard_modularity, 6)),
            weak_partition: weak_partition(report),
            resolution: report.resolution,
            files: report.files,
            converged: report.converged,
            communities: communities.collect(),
        }
    }
}

impl Listing for CommunitiesJson<'_> {
    fn items(&self) -> usize {
        self.communities.len()
    }

    fn prefix(&self, kept: usize) -> Self {
        let mut prefix = self.clone();
        prefix.communities.truncate(kept);
        prefix
    }
}
