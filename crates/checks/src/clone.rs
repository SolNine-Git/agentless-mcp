//! `clone_delta`: each block a change adds that has a Type-2 copy in the repository after it.

use std::collections::{BTreeSet, HashSet};

use prompts::messages;

use crate::copies::Run;
use crate::error::Error;
use crate::finding::{CheckGap, CloneCopy, CloneSite, Detail, Finding, PairFinding, Severity};
use crate::finding::{not_checked, sorted};
use crate::index::{CloneIndex, IndexedFiles};
use crate::observed::{CloneState, Observed, Outcome};
use crate::pair::{CLONE_GAPS, Pair, each_pair, read_sides};

// The detail lists this many instances, then only counts them.
const MAX_CLONE_SITES: usize = 20;
// The evidence names this many sites. The message count stays exact.
const MAX_DUPLICATE_SITES: usize = 3;
const MAX_NAMED_GAPS: usize = 10;

/// Report each added block that is big enough to report and has a Type-2 copy, against `index`.
/// `reported` holds the path and line of each function that `near_duplicates` reported in the
/// same run, and such a function gives no second finding.
///
/// # Errors
///
/// Returns an error when two pairs name one path, or when a side fails in a way that is not a
/// parse or tokenizer failure.
pub fn clone_delta(
    pairs: &[&Pair],
    index: &CloneIndex,
    reported: &BTreeSet<(String, u32)>,
) -> Result<Outcome, Error> {
    let files = match index {
        CloneIndex::NoTagCache => {
            return Ok(Outcome {
                findings: no_tag_cache_rows(pairs),
                observed: Observed::default(),
            });
        }
        CloneIndex::Built(files) => files,
    };
    let (read, rows) = each_pair(pairs, |pair, language| {
        read_sides(pair, language, &CLONE_GAPS)
    })?;
    let changed: HashSet<&str> = pairs.iter().map(|pair| pair.path()).collect();
    let run = Run::new(files, &read, &changed);
    let copies: Vec<usize> = run
        .added()
        .into_iter()
        .filter(|id| run.reportable(*id))
        .collect();
    let mut findings = index_rows(files, &changed);
    findings.extend(rows);
    findings.extend(
        run.outermost(copies, reported)
            .into_iter()
            .map(|id| clone_finding(&run, id)),
    );
    let observed = if read.is_empty() {
        Observed::default()
    } else {
        Observed {
            clone_instances: Some(twinned(&run)),
            ..Observed::default()
        }
    };
    Ok(Outcome {
        findings: sorted(findings),
        observed,
    })
}

fn clone_finding(run: &Run<'_>, id: usize) -> PairFinding {
    let placed = run.placed(id);
    let instances = run.instances(id);
    let mut others: Vec<usize> = instances
        .iter()
        .copied()
        .filter(|other| *other != id)
        .collect();
    run.sort_by_place(&mut others);
    let sites: Vec<String> = others
        .iter()
        .map(|other| {
            format!(
                "{}:{}",
                run.placed(*other).path,
                run.placed(*other).block.start_line
            )
        })
        .collect();
    let (start, end) = (placed.block.start_line, placed.block.end_line);
    let detail = CloneCopy {
        key: placed.block.key,
        line: start,
        end_line: end,
        sites: others
            .iter()
            .take(MAX_CLONE_SITES)
            .map(|other| site(run, *other))
            .collect(),
        sites_total: others.len(),
    };
    let message = messages::CLONE_NEW_COPY.render((start as usize, end as usize, instances.len()));
    let evidence = messages::CLONE_NEW_COPY_EVIDENCE.render(&listed(&sites, MAX_DUPLICATE_SITES));
    let finding = Finding {
        check: crate::CLONE_DELTA,
        severity: Severity::Advisory,
        message,
        path: placed.path.to_owned(),
        line: start,
        evidence,
    };
    PairFinding {
        finding,
        detail: Some(Detail::Copy(detail)),
    }
}

fn site(run: &Run<'_>, id: usize) -> CloneSite {
    let placed = run.placed(id);
    CloneSite {
        path: placed.path.to_owned(),
        line: placed.block.start_line,
        end_line: placed.block.end_line,
    }
}

fn twinned(run: &Run<'_>) -> Vec<CloneState> {
    let mut states: Vec<CloneState> = run
        .new_blocks()
        .filter(|id| run.reportable(*id))
        .map(|id| {
            let placed = run.placed(id);
            CloneState {
                path: placed.path.to_owned(),
                key: placed.block.key,
                line: placed.block.start_line,
                end_line: placed.block.end_line,
            }
        })
        .collect();
    states.sort_by(|a, b| {
        (&a.path, a.line, a.end_line, a.key).cmp(&(&b.path, b.line, b.end_line, b.key))
    });
    states
}

fn listed(items: &[String], limit: usize) -> String {
    let shown = items
        .iter()
        .take(limit)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() <= limit {
        return shown;
    }
    messages::CLONE_AND_MORE.render((items.len() - limit, &shown))
}

fn index_rows(files: &IndexedFiles, changed: &HashSet<&str>) -> Vec<PairFinding> {
    let mut rows = Vec::new();
    if let Some(reason) = &files.unlisted {
        let message = messages::CLONE_INDEX_UNLISTED.render(reason);
        rows.push(not_checked(crate::CLONE_DELTA, "", message, reason, None));
    }
    let gaps: Vec<(&String, &String)> = files
        .unindexed
        .iter()
        .filter(|(path, _)| !changed.contains(path.as_str()))
        .collect();
    if let Some((first, reason)) = gaps.first() {
        let indexed = files
            .files
            .keys()
            .filter(|path| !changed.contains(path.as_str()));
        let total = gaps.len() + indexed.count();
        let paths: Vec<String> = gaps.iter().map(|(path, _)| (*path).clone()).collect();
        let named = listed(&paths, MAX_NAMED_GAPS);
        let message = messages::CLONE_INDEX_GAPS.render((&named, gaps.len(), total));
        let evidence =
            messages::CLONE_INDEX_GAPS_EVIDENCE.render((first.as_str(), reason.as_str()));
        rows.push(not_checked(
            crate::CLONE_DELTA,
            "",
            message,
            &evidence,
            None,
        ));
    }
    rows
}

fn no_tag_cache_rows(pairs: &[&Pair]) -> Vec<PairFinding> {
    if pairs.iter().all(|pair| pair.language().is_none()) {
        return Vec::new();
    }
    let message = messages::CLONE_NO_TAG_CACHE.to_owned();
    let evidence = messages::CLONE_NO_TAG_CACHE_EVIDENCE;
    let gap = Some(CheckGap::NoTagCache);
    vec![not_checked(crate::CLONE_DELTA, "", message, evidence, gap)]
}
