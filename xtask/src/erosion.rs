use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::process::{self, Finished};
use crate::{Failure, repo, verdict};

const BASELINE: &str = "crates/erosion-baseline.json";
const CHECKS: &str = "complexity_delta,clone_delta,contract_drift";
const FAIL_ON: &str = "warning,advisory,not_checked";
// `lint --fail-on` exits 3 when a finding has a listed severity; 1 and 2 mean the run broke.
const LINT_GATE_EXIT: i32 = 3;

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
    complexity: Figures,
}

struct Decision {
    problems: Vec<String>,
    write: Option<Figures>,
}

/// Fail when the pinned lint finds anything since the base, or a health figure rises above the base's baseline.
pub fn check(root: &Path, flags: &[String]) -> Result<(), Failure> {
    let options = parse(flags)?;
    let base = commit(root, options.base.as_deref())?;
    let mut command = Command::new(&options.lint_bin);
    process::stdout(
        command.args(["index", "--repo"]).arg(root),
        process::GATE_BINARY,
    )?;
    let mut problems = if unchanged_since(root, &base)? {
        // lint reports an empty change set as not_checked, which the gate would count as a finding.
        eprintln!("xtask: nothing changed since {base}, so lint has nothing to compare");
        Vec::new()
    } else {
        lint(root, &options.lint_bin, &base)?
    };
    let decision = ratchet(
        base_baseline(root, &base)?,
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

fn parse(flags: &[String]) -> Result<Options, Failure> {
    let mut lint_bin = None;
    let mut health_bin = None;
    let mut base = None;
    let mut update = false;
    let mut rest = flags.iter();
    while let Some(flag) = rest.next() {
        match flag.as_str() {
            "--lint-bin" => lint_bin = Some(PathBuf::from(value(&mut rest)?)),
            "--health-bin" => health_bin = Some(PathBuf::from(value(&mut rest)?)),
            "--base" => base = Some(value(&mut rest)?.to_owned()),
            "--update" => update = true,
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
        update,
    })
}

fn value<'a>(rest: &mut impl Iterator<Item = &'a String>) -> Result<&'a str, Failure> {
    rest.next()
        .map(String::as_str)
        .filter(|value| !value.starts_with("--"))
        .ok_or(Failure::Usage)
}

fn commit(root: &Path, base: Option<&str>) -> Result<String, Failure> {
    let revision = match base {
        Some(base) => base.to_owned(),
        None => merge_base(root)?,
    };
    let mut command = Command::new("git");
    command
        .args(["rev-parse", "--verify", "--end-of-options"])
        .arg(format!("{revision}^{{commit}}"))
        .current_dir(root);
    let output = process::stdout(&mut command, process::GIT)?;
    Ok(String::from_utf8_lossy(&output).trim().to_owned())
}

fn merge_base(root: &Path) -> Result<String, Failure> {
    let mut command = Command::new("git");
    command
        .args(["merge-base", "HEAD", "origin/main"])
        .current_dir(root);
    let output = process::stdout(&mut command, process::GIT)?;
    Ok(String::from_utf8_lossy(&output).trim().to_owned())
}

fn unchanged_since(root: &Path, base: &str) -> Result<bool, Failure> {
    let mut diff = Command::new("git");
    diff.args(["diff", "--quiet", base, "--"]).current_dir(root);
    let status = process::run(&mut diff, process::GIT)?.status;
    let tracked_unchanged = match status.code() {
        Some(0) => true,
        Some(1) => false,
        _ => return Err(Failure::Broken(format!("{diff:?} exited with {status}"))),
    };
    let mut untracked = Command::new("git");
    untracked
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(root);
    Ok(tracked_unchanged && process::stdout(&mut untracked, process::GIT)?.is_empty())
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
    let finished = process::run(&mut command, process::GATE_BINARY)?;
    forward(&finished)?;
    let status = finished.status;
    match status.code() {
        Some(0) => Ok(Vec::new()),
        Some(LINT_GATE_EXIT) => Ok(vec![format!(
            "lint found erosion since {base}: the findings are listed above"
        )]),
        _ => Err(Failure::Broken(format!("{command:?} exited with {status}"))),
    }
}

fn forward(finished: &Finished) -> Result<(), Failure> {
    std::io::stdout()
        .write_all(&finished.stdout)
        .and_then(|()| std::io::stderr().write_all(&finished.stderr))
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
    Ok(health.complexity)
}

fn base_baseline(root: &Path, base: &str) -> Result<Option<Figures>, Failure> {
    let mut listing = Command::new("git");
    listing
        .args(["ls-tree", "-z", "--name-only", base, "--", BASELINE])
        .current_dir(root);
    if process::stdout(&mut listing, process::GIT)?.is_empty() {
        return Ok(None);
    }
    let blob = format!("{base}:{BASELINE}");
    let mut show = Command::new("git");
    show.args(["show", &blob]).current_dir(root);
    figures(&process::stdout(&mut show, process::GIT)?, &blob).map(Some)
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

fn ratchet(
    base: Option<Figures>,
    tree: Option<Figures>,
    current: Figures,
    update: bool,
) -> Decision {
    let (mut problems, reference) = reference(base, tree);
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

fn reference(base: Option<Figures>, tree: Option<Figures>) -> (Vec<String>, Option<Figures>) {
    match (base, tree) {
        (None, tree) => (Vec::new(), tree),
        (Some(base), None) => (
            vec![format!(
                "{BASELINE} is missing, but the base commit has one: restore it"
            )],
            Some(base),
        ),
        (Some(base), Some(tree)) => {
            let raised = base
                .rises(tree)
                .into_iter()
                .map(|rise| format!("{BASELINE} is above the base's: {rise}"))
                .collect();
            (raised, Some(base.lowered(tree)))
        }
    }
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

#[cfg(test)]
mod tests {
    use super::{Figures, parse, ratchet};
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
    fn a_flag_without_a_value_is_a_usage_error() {
        for words in [
            &["--lint-bin", "a", "--health-bin", "b", "--base"][..],
            &["--lint-bin", "a", "--health-bin", "b", "--base", "--update"],
            &["--health-bin", "b", "--lint-bin"],
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
    fn the_tree_cannot_raise_the_base_baseline() {
        let decision = ratchet(Some(ZERO), Some(STORED), ZERO, false);
        assert_eq!(
            decision.problems,
            [
                "crates/erosion-baseline.json is above the base's: erosion rose from 0 to 0.2",
                "crates/erosion-baseline.json is above the base's: units over the complexity threshold rose from 0 to 3",
                "crates/erosion-baseline.json is above the base's: clone share rose from 0 to 0.05",
            ]
        );
        let lowered_tree = ratchet(Some(STORED), Some(ZERO), STORED, false);
        assert_eq!(lowered_tree.problems.len(), 3);
    }

    #[test]
    fn a_deleted_baseline_fails_when_the_base_has_one() {
        let decision = ratchet(Some(STORED), None, STORED, false);
        assert_eq!(
            decision.problems,
            ["crates/erosion-baseline.json is missing, but the base commit has one: restore it"]
        );
    }

    #[test]
    fn with_no_base_baseline_the_tree_is_the_reference() {
        assert!(
            ratchet(None, Some(STORED), STORED, false)
                .problems
                .is_empty()
        );
        assert_eq!(ratchet(None, Some(ZERO), STORED, false).problems.len(), 3);
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
}
