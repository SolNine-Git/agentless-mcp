//! The communities of a repository's files: the partition of the weighted file graph, largest
//! community first, cut at the caller's limits.

use graph::{FileGraph, NameIndex, Scopes, Weighting, detect_communities};
use io::ProjectConfig;
use kernel::Bounded;

use crate::bounds::{at_least, within, within_float};
use crate::ops::limits::{DEFAULT_RESOLUTION, MAX_LIMIT, MAX_RESOLUTION};
use crate::scan::RepoFacts;
use crate::{Error, IntArg, Noted, Reading, Services};

/// A communities request: the modularity resolution, and how many communities and members to
/// list.
#[derive(Clone, Debug, PartialEq)]
pub struct CommunitiesRequest {
    /// The modularity resolution, or `None` for the default.
    pub resolution: Option<f64>,
    /// The most communities the answer lists. The count is always complete.
    pub limit: IntArg,
    /// The most member files each community lists.
    pub members: IntArg,
}

/// A whole partition, cut at the limit, with the scores behind it.
#[derive(Clone, Debug, PartialEq)]
pub struct CommunityReport {
    /// The communities the limit kept, largest first, and how many there were.
    pub communities: Bounded<CommunityRow>,
    /// The modularity at the resolution the partition was found at.
    pub modularity: f64,
    /// The modularity of the same partition at resolution 1.
    pub standard_modularity: f64,
    /// The resolution the partition was found at.
    pub resolution: f64,
    /// How many files the graph holds.
    pub files: usize,
    /// Whether the local moving settled before its pass bound.
    pub converged: bool,
}

/// One community: its label, its members cut at the member limit, and its weights.
#[derive(Clone, Debug, PartialEq)]
pub struct CommunityRow {
    /// The label: the directory a strict majority of the members share, or the root label.
    pub label: String,
    /// The member paths in path order, cut at the member limit.
    pub members: Bounded<String>,
    /// The weight of the edges inside the community. Zero means the community has none.
    pub internal_weight: f64,
    /// The summed degree of the members. Zero means the members have no edge.
    pub total_weight: f64,
}

/// Partitions the files of the repository that `reading` names into communities. The scan's
/// warnings come with either outcome.
#[must_use]
pub fn communities(
    services: &Services,
    reading: &Reading<'_>,
    request: &CommunitiesRequest,
) -> Noted<Result<CommunityReport, Error>> {
    let limits = within(&request.limit, 1, MAX_LIMIT, "limit")
        .and_then(|limit| Ok((limit, at_least(&request.members, 1, "members")?)));
    let (limit, members) = match limits {
        Ok(limits) => limits,
        Err(error) => return Noted::new(Err(error)),
    };
    over_scan(services, reading, |facts| {
        let resolution = resolution_setting(request.resolution)?;
        let config = &reading.repo.config;
        Ok(report(facts, config, resolution, limit, members))
    })
}

// Scans the repository once and gives the scan to `answer`. The scan's warnings come with
// either outcome.
pub(crate) fn over_scan<T>(
    services: &Services,
    reading: &Reading<'_>,
    answer: impl FnOnce(&RepoFacts) -> Result<T, Error>,
) -> Noted<Result<T, Error>> {
    RepoFacts::load(services, reading).map(|loaded| loaded.and_then(|facts| answer(&facts)))
}

fn report(
    facts: &RepoFacts,
    config: &ProjectConfig,
    resolution: f64,
    limit: usize,
    members: usize,
) -> CommunityReport {
    let index = NameIndex::build(&facts.files);
    let graph = file_graph(&index, &Scopes::build(&facts.files), config);
    let partition = detect_communities(&graph, resolution);
    let rows = (partition.communities.into_iter())
        .map(|community| CommunityRow {
            label: community.label,
            members: Bounded::cut(
                community.members.into_iter().map(str::to_owned).collect(),
                members,
            ),
            internal_weight: community.internal_weight,
            total_weight: community.total_weight,
        })
        .collect();
    CommunityReport {
        communities: Bounded::cut(rows, limit),
        modularity: partition.modularity,
        standard_modularity: partition.standard_modularity,
        resolution: partition.resolution,
        files: graph.nodes().len(),
        converged: partition.converged,
    }
}

// The file graph of one scan under the repository's weighting: the graph that communities,
// diagrams and the HTML export draw.
pub(crate) fn file_graph<'a>(
    index: &NameIndex<'a>,
    scopes: &Scopes<'a>,
    config: &ProjectConfig,
) -> FileGraph<'a> {
    let weighting = Weighting {
        relation_weights: config.relation_weights.unwrap_or(false),
        stoplist: &config.stoplist,
    };
    FileGraph::build(index, scopes, &weighting)
}

// NaN makes every gain comparison false and a negative resolution reports a modularity above 1,
// so both are refused. The floor is exclusive, unlike the range check after it.
pub(crate) fn resolution_setting(resolution: Option<f64>) -> Result<f64, Error> {
    let Some(setting) = resolution else {
        return Ok(DEFAULT_RESOLUTION);
    };
    if !setting.is_finite() || setting <= 0.0 {
        let message = format!(
            "resolution must be a finite number greater than 0, got {}",
            json::float_text(setting)
        );
        return Err(kernel::Error::OperationFailed(message).into());
    }
    within_float(setting, 0.0, MAX_RESOLUTION, "resolution")
}

#[cfg(test)]
mod tests;
