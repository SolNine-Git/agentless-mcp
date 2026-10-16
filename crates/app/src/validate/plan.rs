//! The planning phase: each candidate's edits read, guarded and normalized against HEAD in one
//! worktree, then grouped by the exact files they produce.

use std::collections::{BTreeSet, HashMap};

use git::Worktree;
use json::JsonForm;
use patch::Edit;
use sha2::{Digest, Sha256};

use super::built::{apply_failed, reasons};
use super::candidates::Candidate;
use super::records::{ApplyStatus, CandidateVerdict};
use super::{Run, worktree_repo};
use crate::patches::{NormalizeReport, load_edits, normalize_patch};
use crate::{Error, Noted, Repo};

// The files that decide what the test command runs rather than what the code does, matched on
// the basename anywhere in the tree, and the prefixes whose whole subtree decides what CI runs.
const TEST_CONFIG_NAMES: [&str; 14] = [
    "conftest.py",
    "pytest.ini",
    "tox.ini",
    "noxfile.py",
    "setup.cfg",
    "setup.py",
    "pyproject.toml",
    "Makefile",
    "makefile",
    "GNUmakefile",
    "package.json",
    ".pre-commit-config.yaml",
    "sitecustomize.py",
    "usercustomize.py",
];
const TEST_CONFIG_PREFIXES: [&str; 1] = [".github/"];

// One candidate normalized against HEAD before any command is scheduled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Plan<'c> {
    pub(crate) candidate: &'c Candidate,
    pub(crate) edits: Vec<Edit>,
    pub(crate) equivalence_key: Option<String>,
    pub(crate) execution_key: Vec<(String, String)>,
    pub(crate) execution_group: String,
}

// The plans, and the verdicts of the candidates that can never apply.
pub(crate) type Planned<'c> = (Vec<Plan<'c>>, Vec<CandidateVerdict>);

// A candidate that can be scheduled, or the reasons it cannot apply at all.
enum Planning<'c> {
    Scheduled(Plan<'c>),
    Refused(Vec<String>),
}

impl Run<'_> {
    pub(super) fn plan_candidates<'c>(
        &self,
        candidates: &'c [Candidate],
    ) -> Noted<Result<Planned<'c>, Error>> {
        self.in_worktree(|tree| Noted::new(self.planned(tree, candidates)))
    }

    // A path outside the worktree is a refused invocation, never a failed candidate.
    fn planned<'c>(
        &self,
        tree: &Worktree,
        candidates: &'c [Candidate],
    ) -> Result<Planned<'c>, Error> {
        let repo = worktree_repo(tree)?;
        let mut plans = Vec::new();
        let mut failures = Vec::new();
        for candidate in candidates {
            match self.planning(&repo, candidate)? {
                Planning::Scheduled(plan) => plans.push(plan),
                Planning::Refused(reasons) => {
                    let started = self.now();
                    failures.push(apply_failed(candidate, reasons, self.elapsed(started)));
                }
            }
        }
        Ok((plans, failures))
    }

    fn planning<'c>(&self, repo: &Repo, candidate: &'c Candidate) -> Result<Planning<'c>, Error> {
        let allowed = self.request.allow_test_config_edits;
        let edits = match candidate_edits(&candidate.text, allowed) {
            Ok(edits) => edits,
            Err(reasons) => return Ok(Planning::Refused(reasons)),
        };
        let normalized = normalize_patch(repo, &edits)?;
        if !normalized.ok() {
            return Ok(Planning::Refused(reasons(&normalized.result)));
        }
        Ok(Planning::Scheduled(plan(candidate, edits, normalized)))
    }
}

// Every refusal here comes before a worktree is written, so a candidate that was never going
// to be judged costs no apply and no test run.
fn candidate_edits(text: &str, allow_test_config_edits: bool) -> Result<Vec<Edit>, Vec<String>> {
    let parsed = load_edits(text).map_err(|error| vec![error.to_string()])?;
    if !parsed.errors.is_empty() {
        let reasons = parsed.errors.iter().map(|error| {
            let path = error.path.as_deref().filter(|path| !path.is_empty());
            let path = path.unwrap_or("no path");
            format!("block {} ({path}): {}", error.index, error.reason)
        });
        return Err(reasons.collect());
    }
    if parsed.edits.is_empty() {
        return Err(vec!["the candidate contains no edits".to_owned()]);
    }
    if !allow_test_config_edits {
        let rewritten = test_config_paths(parsed.edits.iter().map(|edit| edit.path.as_str()));
        if !rewritten.is_empty() {
            return Err(vec![test_config_reason(&rewritten)]);
        }
    }
    Ok(parsed.edits)
}

// The given paths that name what the test command runs, sorted and once each. Each path is
// judged as the applier writes it, so `./.github/x` and `src/../.github/x` both land in .github/.
pub(crate) fn test_config_paths<'p>(paths: impl Iterator<Item = &'p str>) -> Vec<&'p str> {
    let named: BTreeSet<&str> = paths.filter(|path| names_test_config(path)).collect();
    named.into_iter().collect()
}

fn names_test_config(path: &str) -> bool {
    let canonical = graph::normpath(path);
    let name = canonical.rsplit('/').next().unwrap_or_default();
    TEST_CONFIG_NAMES.contains(&name)
        || TEST_CONFIG_PREFIXES
            .iter()
            .any(|prefix| canonical.starts_with(prefix))
}

pub(super) fn test_config_reason(paths: &[&str]) -> String {
    format!(
        "refusing to apply edits to {}: those files name what the test command runs, so the \
         candidate would be choosing how it is judged. Opt in with --allow-test-config-edits.",
        paths.join(", ")
    )
}

fn plan(candidate: &Candidate, edits: Vec<Edit>, normalized: NormalizeReport) -> Plan<'_> {
    let mut execution_key = normalized.result.new_contents;
    execution_key.sort_unstable();
    let equivalence_key = (!execution_key.is_empty()).then_some(normalized.key);
    Plan {
        candidate,
        edits,
        equivalence_key,
        execution_group: fingerprint(&execution_key),
        execution_key,
    }
}

// The hash of the exact changed paths and contents, for the additive verdict receipt.
pub(super) fn fingerprint(execution_key: &[(String, String)]) -> String {
    let document =
        json::to_text(execution_key, JsonForm::Packed).expect("pairs of strings serialize");
    let digest = Sha256::digest(document.as_bytes());
    format!("sha256:{}", kernel::Key::new(digest.into()))
}

// The plans grouped by exact result, each group and each member in first appearance.
pub(crate) fn execution_groups(plans: &[Plan<'_>]) -> Vec<Vec<usize>> {
    let mut slots: HashMap<&[(String, String)], usize> = HashMap::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (index, plan) in plans.iter().enumerate() {
        let slot = *slots
            .entry(plan.execution_key.as_slice())
            .or_insert_with(|| {
                groups.push(Vec::new());
                groups.len() - 1
            });
        groups[slot].push(index);
    }
    groups
}

// One representative's execution as the record of `plan`. The planned key travels only with a
// verdict whose patch applied, and only the representative keeps the duration and the runs.
pub(crate) fn share_execution(
    verdict: &CandidateVerdict,
    plan: &Plan<'_>,
    representative: &str,
) -> CandidateVerdict {
    let applied = verdict.apply_status == ApplyStatus::Ok;
    let key = if applied {
        &plan.equivalence_key
    } else {
        &verdict.equivalence_key
    };
    let shared = CandidateVerdict {
        id: plan.candidate.id.clone(),
        index: plan.candidate.index,
        equivalence_key: key.clone(),
        execution_group: Some(plan.execution_group.clone()),
        executed_as: Some(representative.to_owned()),
        ..verdict.clone()
    };
    if plan.candidate.id == representative {
        return shared;
    }
    CandidateVerdict {
        duration: 0.0,
        regression_run: None,
        reproduction_run: None,
        ..shared
    }
}
