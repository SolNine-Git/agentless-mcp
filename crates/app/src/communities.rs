//! The communities of a repository's files: the partition of the weighted file graph, largest
//! community first, cut at the caller's limits.

use graph::{NameIndex, Scopes, detect_communities};
use kernel::Bounded;

use crate::bounds::{at_least, within};
use crate::file_graph::{file_graph, resolution_setting};
use crate::ops::limits::MAX_LIMIT;
use crate::scan::{RepoFacts, over_scan};
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
        report(facts, reading, resolution, limit, members)
    })
}

fn report(
    facts: &RepoFacts,
    reading: &Reading<'_>,
    resolution: f64,
    limit: usize,
    members: usize,
) -> Result<CommunityReport, Error> {
    let (config, deadline) = (&reading.repo.config, reading.deadline);
    let index = NameIndex::build(&facts.files);
    let graph = file_graph(&index, &Scopes::build(&facts.files), config, deadline)?;
    let partition = detect_communities(&graph, resolution, deadline)?;
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
    Ok(CommunityReport {
        communities: Bounded::cut(rows, limit),
        modularity: partition.modularity,
        standard_modularity: partition.standard_modularity,
        resolution: partition.resolution,
        files: graph.nodes().len(),
        converged: partition.converged,
    })
}

#[cfg(test)]
mod tests;
