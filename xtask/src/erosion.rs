use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde::{Deserialize, Serialize};

use crate::{Failure, process, repo, verdict};

const BASELINE: &str = "crates/erosion-baseline.json";
const CHECKS: &str = "complexity_delta,clone_delta,contract_drift";
const FAIL_ON: &str = "warning,advisory,not_checked";
// `lint --fail-on` exits 3 when a finding has a listed severity; 1 and 2 mean the run broke.
const LINT_GATE_EXIT: i32 = 3;
// over_threshold counts units above this complexity; a config file in the tree must not move it.
const THRESHOLD: u64 = 10;
const HIDDEN_SHOWN: usize = 3;

struct Options {
    lint_bin: PathBuf,
    health_bin: PathBuf,
    base: Option<String>,
    update: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Figures {
    erosion: f64,
    over_threshold: u64,
    clone_share: f64,
}

#[derive(Deserialize)]
struct Health {
    complexity: Complexity,
}

#[derive(Deserialize)]
struct Complexity {
    threshold: u64,
    files: u64,
    walked: u64,
    stopped_by_budget: bool,
    not_measured: NotMeasured,
    erosion: f64,
    over_threshold: u64,
    clone_share: f64,
}

#[derive(Deserialize)]
struct NotMeasured {
    total: u64,
}

struct Decision {
    problems: Vec<String>,
    write: Option<Figures>,
}

/// Fail when the pinned lint finds anything since the base, or health rises above the lowest committed baseline.
pub fn check(root: &Path, flags: &[String]) -> Result<(), Failure> {
    let options = parse(flags)?;
    let base = repo::base(root, options.base.as_deref())?;
    let mut command = Command::new(&options.lint_bin);
    process::stdout(
        command.args(["index", "--repo"]).arg(root),
        process::GATE_BINARY,
    )?;
    let mut problems = changes(root, &options.lint_bin, &base)?;
    let decision = ratchet(
        committed_baseline(root, &base)?,
        tree_baseline(root)?,
        health(root, &options.health_bin)?,
        options.update,
    );
    if let Some(figures) = decision.write {
        write_baseline(root, figures)?;
    }
    problems.extend(decision.problems);
    verdict(problems)
}

fn ratchet(
    committed: Option<Figures>,
    tree: Option<Figures>,
    current: Figures,
    update: bool,
) -> Decision {
    let (mut problems, reference) = reference(committed, tree, current);
    let Some(reference) = reference else {
        if !update {
            problems.push(format!(
                "{BASELINE} is missing: run cargo xtask erosion with --update"
            ));
        }
        return Decision {
            problems,
            write: update.then_some(current),
        };
    };
    problems.extend(reference.rises(current));
    Decision {
        problems,
        write: update.then(|| reference.lowered(current)),
    }
}

fn reference(
    committed: Option<Figures>,
    tree: Option<Figures>,
    current: Figures,
) -> (Vec<String>, Option<Figures>) {
    match (committed, tree) {
        (None, None) => (Vec::new(), None),
        (None, Some(tree)) => (above(tree, current, "what health measures now"), Some(tree)),
        (Some(committed), None) => (
            vec![format!(
                "{BASELINE} is missing, but a committed version exists: restore it"
            )],
            Some(committed),
        ),
        (Some(committed), Some(tree)) => (
            above(tree, committed, "its lowest committed version"),
            Some(committed.lowered(tree)),
        ),
    }
}

fn above(tree: Figures, bound: Figures, name: &str) -> Vec<String> {
    bound
        .rises(tree)
        .into_iter()
        .map(|rise| format!("{BASELINE} is above {name}: {rise}"))
        .collect()
}

fn lowest(versions: Vec<Figures>) -> Option<Figures> {
    versions.into_iter().reduce(Figures::lowered)
}

fn committed_baseline(root: &Path, base: &str) -> Result<Option<Figures>, Failure> {
    let mut revisions = vec![base.to_owned()];
    revisions.extend(repo::touching(root, base, BASELINE)?);
    let mut versions = Vec::new();
    for revision in revisions {
        if let Some(json) = repo::show(root, &revision, BASELINE)? {
            versions.push(figures(&json, &format!("{revision}:{BASELINE}"))?);
        }
    }
    Ok(lowest(versions))
}

fn tree_baseline(root: &Path) -> Result<Option<Figures>, Failure> {
    let path = root.join(BASELINE);
    if !repo::exists(&path)? {
        return Ok(None);
    }
    figures(repo::read(&path)?.as_bytes(), BASELINE).map(Some)
}

fn figures(json: &[u8], source: &str) -> Result<Figures, Failure> {
    serde_json::from_slice(json).map_err(|error| Failure::Broken(format!("{source}: {error}")))
}

fn write_baseline(root: &Path, figures: Figures) -> Result<(), Failure> {
    let text = serde_json::to_string_pretty(&figures)
        .map_err(|error| Failure::Broken(format!("{BASELINE}: {error}")))?;
    repo::write(&root.join(BASELINE), &format!("{text}\n"))
}

impl Figures {
    fn lowered(self, current: Self) -> Self {
        Self {
            erosion: self.erosion.min(current.erosion),
            over_threshold: self.over_threshold.min(current.over_threshold),
            clone_share: self.clone_share.min(current.clone_share),
        }
    }

    fn rises(self, current: Self) -> Vec<String> {
        let mut problems = Vec::new();
        if current.erosion > self.erosion {
            problems.push(format!(
                "erosion rose from {} to {}",
                self.erosion, current.erosion
            ));
        }
        if current.over_threshold > self.over_threshold {
            problems.push(format!(
                "units over the complexity threshold rose from {} to {}",
                self.over_threshold, current.over_threshold
            ));
        }
        if current.clone_share > self.clone_share {
            problems.push(format!(
                "clone share rose from {} to {}",
                self.clone_share, current.clone_share
            ));
        }
        problems
    }
}

fn parse(flags: &[String]) -> Result<Options, Failure> {
    let mut lint_bin = None;
    let mut health_bin = None;
    let mut base = None;
    let mut update = None;
    let mut rest = flags.iter();
    while let Some(flag) = rest.next() {
        match flag.as_str() {
            "--lint-bin" => once(&mut lint_bin, PathBuf::from(value(&mut rest)?))?,
            "--health-bin" => once(&mut health_bin, PathBuf::from(value(&mut rest)?))?,
            "--base" => once(&mut base, value(&mut rest)?.to_owned())?,
            "--update" => once(&mut update, true)?,
            _ => return Err(Failure::Usage),
        }
    }
    let (Some(lint_bin), Some(health_bin)) = (lint_bin, health_bin) else {
        return Err(Failure::Usage);
    };
    Ok(Options {
        lint_bin,
        health_bin,
        base,
        update: update.is_some(),
    })
}

fn once<T>(slot: &mut Option<T>, value: T) -> Result<(), Failure> {
    match slot.replace(value) {
        None => Ok(()),
        Some(_) => Err(Failure::Usage),
    }
}

fn value<'a>(rest: &mut impl Iterator<Item = &'a String>) -> Result<&'a str, Failure> {
    rest.next()
        .map(String::as_str)
        .filter(|value| !value.starts_with("--"))
        .ok_or(Failure::Usage)
}

fn changes(root: &Path, lint_bin: &Path, base: &str) -> Result<Vec<String>, Failure> {
    if let Some(problem) = hidden_problem(&repo::git(root, &["ls-files", "-v", "-z"])?)? {
        return Ok(vec![problem]);
    }
    if unchanged_since(root, base)? {
        // lint reports an empty change set as not_checked, which the gate would count as a finding.
        eprintln!("xtask: nothing changed since {base}, so lint has nothing to compare");
        return Ok(Vec::new());
    }
    lint(root, lint_bin, base)
}

// git diff, and lint's change set that it builds, skip an edit to a file marked assume-unchanged
// (a lowercase tag) or skip-worktree (S).
fn hidden_problem(listing: &[u8]) -> Result<Option<String>, Failure> {
    let text = std::str::from_utf8(listing)
        .map_err(|error| Failure::Broken(format!("git ls-files -v: {error}")))?;
    let mut hidden = Vec::new();
    for entry in text.split('\0').filter(|entry| !entry.is_empty()) {
        let (tag, path) = entry.split_once(' ').ok_or_else(|| {
            Failure::Broken(format!(
                "git ls-files -v printed a malformed entry: {entry:?}"
            ))
        })?;
        if tag == "S" || tag.chars().any(|letter| letter.is_ascii_lowercase()) {
            hidden.push(path);
        }
    }
    Ok((!hidden.is_empty()).then(|| {
        let shown: Vec<&str> = hidden.iter().take(HIDDEN_SHOWN).copied().collect();
        format!(
            "{} tracked files carry an assume-unchanged or skip-worktree mark, so git hides their edits from erosion (first: {}). Clear each mark with git update-index --no-assume-unchanged --no-skip-worktree <path>",
            hidden.len(),
            shown.join(", ")
        )
    }))
}

fn unchanged_since(root: &Path, base: &str) -> Result<bool, Failure> {
    let mut diff = repo::git_command(root);
    diff.args(["diff", "--quiet", base, "--"]);
    let status = process::run(&mut diff, process::GIT)?.status;
    let tracked_unchanged = match status.code() {
        Some(0) => true,
        Some(1) => false,
        _ => {
            return Err(Failure::Broken(format!(
                "{} exited with {status}",
                process::label(&diff)
            )));
        }
    };
    let untracked = repo::git(root, &["ls-files", "--others", "--exclude-standard"])?;
    Ok(tracked_unchanged && untracked.is_empty())
}

fn lint(root: &Path, lint_bin: &Path, base: &str) -> Result<Vec<String>, Failure> {
    let mut command = Command::new(lint_bin);
    command
        .args([
            "lint",
            "--since",
            base,
            "--only",
            CHECKS,
            "--fail-on",
            FAIL_ON,
            "--repo",
        ])
        .arg(root);
    let output = process::run(&mut command, process::GATE_BINARY)?;
    forward(&output)?;
    let status = output.status;
    match status.code() {
        Some(0) => Ok(Vec::new()),
        Some(LINT_GATE_EXIT) => Ok(vec![format!(
            "lint found erosion since {base}: the findings are listed above"
        )]),
        _ => Err(Failure::Broken(format!(
            "{} exited with {status}",
            process::label(&command)
        ))),
    }
}

fn forward(output: &Output) -> Result<(), Failure> {
    std::io::stdout()
        .write_all(&output.stdout)
        .and_then(|()| std::io::stderr().write_all(&output.stderr))
        .map_err(|error| Failure::Broken(format!("cannot forward the lint output: {error}")))
}

fn health(root: &Path, health_bin: &Path) -> Result<Figures, Failure> {
    let mut command = Command::new(health_bin);
    command
        .args(["health", "--json", "--repo"])
        .arg(root.join("crates"));
    let output = process::stdout(&mut command, process::GATE_BINARY)?;
    let health: Health = serde_json::from_slice(&output)
        .map_err(|error| Failure::Broken(format!("health --json: {error}")))?;
    let gaps = health.complexity.gaps();
    if gaps.is_empty() {
        Ok(health.complexity.figures())
    } else {
        Err(Failure::Broken(format!(
            "health --json cannot judge the change: {}",
            gaps.join("; ")
        )))
    }
}

impl Complexity {
    fn gaps(&self) -> Vec<String> {
        let mut gaps = Vec::new();
        if self.threshold != THRESHOLD {
            gaps.push(format!(
                "it counted units over complexity {}, not the pinned {THRESHOLD}",
                self.threshold
            ));
        }
        if self.stopped_by_budget {
            gaps.push("its time budget stopped the walk".to_owned());
        }
        if self.walked != self.files {
            gaps.push(format!("it walked {} of {} files", self.walked, self.files));
        }
        if self.not_measured.total > 0 {
            gaps.push(format!(
                "it could not measure {} files",
                self.not_measured.total
            ));
        }
        gaps
    }

    fn figures(&self) -> Figures {
        Figures {
            erosion: self.erosion,
            over_threshold: self.over_threshold,
            clone_share: self.clone_share,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Figures, Health, hidden_problem, lowest, parse, ratchet};
    use crate::Failure;

    const STORED: Figures = Figures {
        erosion: 0.2,
        over_threshold: 3,
        clone_share: 0.05,
    };
    const ZERO: Figures = Figures {
        erosion: 0.0,
        over_threshold: 0,
        clone_share: 0.0,
    };

    fn flags(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    fn health(threshold: u64, walked: u64, stopped: bool, not_measured: u64) -> Health {
        let json = format!(
            r#"{{"complexity": {{"threshold": {threshold}, "files": 7, "walked": {walked},
            "measured": 7, "stopped_by_budget": {stopped}, "units": 38, "over_threshold": 0,
            "erosion": 0.0, "sloc": 382, "clone_lines": 12, "clone_share": 0.03,
            "not_measured": {{"total": {not_measured}, "limit": 20, "omitted": 0, "rows": []}}}}}}"#
        );
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn a_figure_that_rises_is_reported_and_one_that_falls_is_not() {
        let current = Figures {
            erosion: 0.1,
            over_threshold: 4,
            clone_share: 0.05,
        };
        assert_eq!(
            STORED.rises(current),
            ["units over the complexity threshold rose from 3 to 4"]
        );
    }

    #[test]
    fn an_update_keeps_the_lower_of_each_figure() {
        let current = Figures {
            erosion: 0.3,
            over_threshold: 1,
            clone_share: 0.04,
        };
        let lowered = STORED.lowered(current);
        assert_eq!(
            lowered,
            Figures {
                erosion: 0.2,
                over_threshold: 1,
                clone_share: 0.04
            }
        );
    }

    #[test]
    fn a_flag_without_a_value_or_given_twice_is_a_usage_error() {
        for words in [
            &["--lint-bin", "a", "--health-bin", "b", "--base"][..],
            &["--lint-bin", "a", "--health-bin", "b", "--base", "--update"],
            &["--health-bin", "b", "--lint-bin"],
            &[
                "--base",
                "X",
                "--base",
                "Y",
                "--lint-bin",
                "a",
                "--health-bin",
                "b",
            ],
            &["--lint-bin", "a", "--lint-bin", "c", "--health-bin", "b"],
            &[
                "--update",
                "--update",
                "--lint-bin",
                "a",
                "--health-bin",
                "b",
            ],
        ] {
            assert!(
                matches!(parse(&flags(words)), Err(Failure::Usage)),
                "{words:?}"
            );
        }
        let options = parse(&flags(&[
            "--lint-bin",
            "a",
            "--health-bin",
            "b",
            "--base",
            "HEAD",
        ]));
        assert!(options.is_ok_and(|options| options.base.as_deref() == Some("HEAD")));
    }

    #[test]
    fn the_tree_cannot_raise_the_lowest_committed_baseline() {
        let decision = ratchet(Some(ZERO), Some(STORED), ZERO, false);
        assert_eq!(
            decision.problems,
            [
                "crates/erosion-baseline.json is above its lowest committed version: erosion rose from 0 to 0.2",
                "crates/erosion-baseline.json is above its lowest committed version: units over the complexity threshold rose from 0 to 3",
                "crates/erosion-baseline.json is above its lowest committed version: clone share rose from 0 to 0.05",
            ]
        );
        let lowered_tree = ratchet(Some(STORED), Some(ZERO), STORED, false);
        assert_eq!(lowered_tree.problems.len(), 3);
    }

    #[test]
    fn the_reference_is_the_lowest_of_every_committed_version() {
        let mixed = Figures {
            erosion: 0.1,
            over_threshold: 5,
            clone_share: 0.0,
        };
        let expected = Figures {
            erosion: 0.1,
            over_threshold: 3,
            clone_share: 0.0,
        };
        assert_eq!(lowest(vec![STORED, mixed]), Some(expected));
        assert_eq!(lowest(Vec::new()), None);
    }

    #[test]
    fn a_deleted_baseline_fails_when_a_committed_version_exists() {
        let decision = ratchet(Some(STORED), None, STORED, false);
        assert_eq!(
            decision.problems,
            ["crates/erosion-baseline.json is missing, but a committed version exists: restore it"]
        );
    }

    #[test]
    fn with_no_committed_baseline_the_tree_must_not_exceed_what_health_measures() {
        assert!(
            ratchet(None, Some(STORED), STORED, false)
                .problems
                .is_empty()
        );
        let inflated = ratchet(None, Some(STORED), ZERO, false);
        assert_eq!(inflated.problems.len(), 3);
        assert!(inflated.problems[0].contains("is above what health measures now"));
        let rise = Figures {
            erosion: 0.5,
            ..STORED
        };
        let updated = ratchet(None, Some(STORED), rise, true);
        assert_eq!(updated.problems, ["erosion rose from 0.2 to 0.5"]);
        assert_eq!(updated.write, Some(STORED));
    }

    #[test]
    fn update_writes_the_lower_figures_and_keeps_the_verdict() {
        let current = Figures {
            erosion: 0.3,
            over_threshold: 1,
            clone_share: 0.04,
        };
        let decision = ratchet(Some(STORED), Some(STORED), current, true);
        assert_eq!(decision.problems, ["erosion rose from 0.2 to 0.3"]);
        assert_eq!(
            decision.write,
            Some(Figures {
                erosion: 0.2,
                over_threshold: 1,
                clone_share: 0.04
            })
        );
        assert_eq!(
            ratchet(Some(STORED), Some(STORED), current, false).write,
            None
        );
    }

    #[test]
    fn a_file_whose_edits_git_hides_fails_and_a_plain_listing_passes() {
        let plain = b"H Cargo.toml\0M crates/a.rs\0C crates/b.rs\0";
        assert!(hidden_problem(plain).unwrap().is_none());
        let marked = b"H Cargo.toml\0h crates/a.rs\0S crates/b.rs\0s c\0H d\0S e\0";
        let problem = hidden_problem(marked).unwrap().unwrap();
        assert!(
            problem.starts_with("4 tracked files carry an assume-unchanged or skip-worktree mark"),
            "{problem}"
        );
        assert!(
            problem.contains("(first: crates/a.rs, crates/b.rs, c)"),
            "{problem}"
        );
        assert!(matches!(
            hidden_problem(b"no-tag\0"),
            Err(Failure::Broken(_))
        ));
    }

    #[test]
    fn no_baseline_anywhere_needs_update() {
        let decision = ratchet(None, None, STORED, false);
        assert_eq!(
            decision.problems,
            ["crates/erosion-baseline.json is missing: run cargo xtask erosion with --update"]
        );
        let bootstrap = ratchet(None, None, STORED, true);
        assert!(bootstrap.problems.is_empty());
        assert_eq!(bootstrap.write, Some(STORED));
    }

    #[test]
    fn a_partial_or_rethresholded_health_walk_cannot_judge_the_change() {
        assert!(health(10, 7, false, 0).complexity.gaps().is_empty());
        assert_eq!(
            health(100, 3, true, 2).complexity.gaps(),
            [
                "it counted units over complexity 100, not the pinned 10",
                "its time budget stopped the walk",
                "it walked 3 of 7 files",
                "it could not measure 2 files",
            ]
        );
    }
}
