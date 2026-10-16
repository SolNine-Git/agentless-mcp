//! The filter ladder and the vote over equivalence clusters of validated candidates.

use std::cmp::Reverse;
use std::collections::HashMap;

use serde::Serialize;
use serde::ser::{SerializeStruct, Serializer};

/// The rung of the ladder that answered, the strongest evidence first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Reproduction,
    Regression,
    Applied,
    None,
}

impl Tier {
    // In the order of the variants.
    const NAMES: [&'static str; 4] = ["regression+reproduction", "regression", "applied", "none"];
    const DETAILS: [&'static str; 4] = [
        "fixed the reproduction test and broke no regression test",
        "broke no regression test (nothing fixed the reproduction test)",
        "applied cleanly (nothing passed the regression suite)",
        "no candidate applied cleanly with a usable equivalence key",
    ];

    /// Returns the tier as the reports spell it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        Self::NAMES[self as usize]
    }

    /// Returns what the candidates of this tier did.
    #[must_use]
    pub fn detail(self) -> &'static str {
        Self::DETAILS[self as usize]
    }
}

/// What the regression command gave one candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Regression {
    /// The command never ran for the candidate, or could not start.
    NotMeasured,
    /// The command ran and failed or outlived its bound.
    Failed,
    Passed,
}

/// One validated candidate, reduced to what the ladder and the vote read. `index` is its place
/// in first-appearance order and breaks ties between clusters of one size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoteCandidate {
    pub id: String,
    pub index: i64,
    pub applied: bool,
    pub equivalence_key: Option<String>,
    pub regression: Regression,
    pub reproduction_passed: bool,
}

impl VoteCandidate {
    fn regression_passed(&self) -> bool {
        self.regression == Regression::Passed
    }
}

/// One equivalence class of candidates; its representative is its first-appearing member.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Cluster {
    pub rank: usize,
    pub key: String,
    pub size: usize,
    pub members: Vec<String>,
    pub representative: String,
    pub first_appearance: i64,
    pub regression_passed: usize,
    pub reproduction_passed: usize,
}

/// A candidate that cannot vote, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Exclusion {
    pub id: String,
    pub reason: &'static str,
}

/// The tier that answered, and the clusters in rank order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoteReport {
    pub tier: Tier,
    pub repro_valid: bool,
    pub considered: usize,
    pub survived: usize,
    pub clusters: Vec<Cluster>,
    pub excluded: Vec<Exclusion>,
}

impl VoteReport {
    /// Returns the representative of the top cluster, or `None` when nothing ranked.
    #[must_use]
    pub fn winner(&self) -> Option<&str> {
        self.clusters
            .first()
            .map(|cluster| cluster.representative.as_str())
    }
}

impl Serialize for VoteReport {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut form = serializer.serialize_struct("VoteReport", 8)?;
        form.serialize_field("tier", self.tier.as_str())?;
        form.serialize_field("tier_detail", self.tier.detail())?;
        form.serialize_field("repro_valid", &self.repro_valid)?;
        form.serialize_field("considered", &self.considered)?;
        form.serialize_field("survived", &self.survived)?;
        form.serialize_field("winner", &self.winner())?;
        form.serialize_field("clusters", &self.clusters)?;
        form.serialize_field("excluded", &self.excluded)?;
        form.end()
    }
}

/// Narrows the candidates to the strongest tier that is not empty, then ranks its equivalence
/// clusters by size, ties broken by first appearance. Without a valid reproduction test the
/// reproduction rung leaves the ladder.
#[must_use]
pub fn rank(candidates: &[VoteCandidate], repro_valid: bool) -> VoteReport {
    let mut ordered: Vec<&VoteCandidate> = candidates.iter().collect();
    ordered.sort_by_key(|candidate| candidate.index);
    let mut eligible = Vec::new();
    let mut excluded = Vec::new();
    for candidate in ordered {
        match exclusion(candidate) {
            None => eligible.push(candidate),
            Some(reason) => excluded.push(Exclusion {
                id: candidate.id.clone(),
                reason,
            }),
        }
    }
    let (tier, survivors) = ladder(&eligible, repro_valid);
    VoteReport {
        tier,
        repro_valid,
        considered: candidates.len(),
        survived: survivors.len(),
        clusters: clusters(&survivors),
        excluded,
    }
}

fn exclusion(candidate: &VoteCandidate) -> Option<&'static str> {
    if !candidate.applied {
        return Some("the patch did not apply");
    }
    if candidate.regression == Regression::NotMeasured {
        return Some("the test command never ran for it, so nothing was measured");
    }
    if candidate
        .equivalence_key
        .as_deref()
        .is_none_or(str::is_empty)
    {
        return Some("the patch applied but changed nothing, so it has no equivalence key");
    }
    None
}

type Rung = (Tier, fn(&VoteCandidate) -> bool);

fn ladder<'a>(eligible: &[&'a VoteCandidate], repro_valid: bool) -> (Tier, Vec<&'a VoteCandidate>) {
    let rungs: [Rung; 3] = [
        (Tier::Reproduction, |candidate| {
            candidate.regression_passed() && candidate.reproduction_passed
        }),
        (Tier::Regression, VoteCandidate::regression_passed),
        (Tier::Applied, |_| true),
    ];
    let usable = rungs
        .into_iter()
        .filter(|(tier, _)| repro_valid || *tier != Tier::Reproduction);
    for (tier, keeps) in usable {
        let survivors: Vec<&VoteCandidate> = eligible
            .iter()
            .copied()
            .filter(|candidate| keeps(candidate))
            .collect();
        if !survivors.is_empty() {
            return (tier, survivors);
        }
    }
    (Tier::None, Vec::new())
}

// The survivors by key, the largest cluster first, ties to the earliest first appearance.
fn clusters(survivors: &[&VoteCandidate]) -> Vec<Cluster> {
    let mut grouped: Vec<(&str, Vec<&VoteCandidate>)> = Vec::new();
    let mut slots: HashMap<&str, usize> = HashMap::new();
    for &candidate in survivors {
        let key = candidate.equivalence_key.as_deref().unwrap_or_default();
        let slot = *slots.entry(key).or_insert_with(|| {
            grouped.push((key, Vec::new()));
            grouped.len() - 1
        });
        grouped[slot].1.push(candidate);
    }
    grouped.sort_by_key(|(_, members)| {
        let first = members.iter().map(|member| member.index).min();
        (Reverse(members.len()), first)
    });
    grouped
        .into_iter()
        .zip(1..)
        .map(|((key, members), position)| cluster(position, key, members))
        .collect()
}

fn cluster(rank: usize, key: &str, mut members: Vec<&VoteCandidate>) -> Cluster {
    members.sort_by_key(|member| member.index);
    let count =
        |passed: fn(&VoteCandidate) -> bool| members.iter().filter(|member| passed(member)).count();
    Cluster {
        rank,
        key: key.to_owned(),
        size: members.len(),
        members: members.iter().map(|member| member.id.clone()).collect(),
        representative: members[0].id.clone(),
        first_appearance: members[0].index,
        regression_passed: count(VoteCandidate::regression_passed),
        reproduction_passed: count(|member| member.reproduction_passed),
    }
}
