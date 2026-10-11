use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

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

/// Fail when the pinned lint finds anything since the base, or a health figure rises above the baseline.
pub fn check(root: &Path, flags: &[String]) -> Result<(), Failure> {
    let options = parse(flags)?;
    let base = match &options.base {
        Some(base) => base.clone(),
        None => merge_base(root)?,
    };
    let mut command = Command::new(&options.lint_bin);
    repo::stdout(command.args(["index", "--repo"]).arg(root))?;
    let mut problems = if unchanged_since(root, &base)? {
        // lint reports an empty change set as not_checked, which the gate would count as a finding.
        eprintln!("xtask: nothing changed since {base}, so lint has nothing to compare");
        Vec::new()
    } else {
        lint(root, &options.lint_bin, &base)?
    };
    let current = health(root, &options.health_bin)?;
    problems.extend(ratchet(root, current, options.update)?);
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
            "--lint-bin" => lint_bin = rest.next().map(PathBuf::from),
            "--health-bin" => health_bin = rest.next().map(PathBuf::from),
            "--base" => base = rest.next().cloned(),
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

fn merge_base(root: &Path) -> Result<String, Failure> {
    let mut command = Command::new("git");
    command
        .args(["merge-base", "HEAD", "origin/main"])
        .current_dir(root);
    let output = repo::stdout(&mut command)?;
    Ok(String::from_utf8_lossy(&output).trim().to_owned())
}

fn unchanged_since(root: &Path, base: &str) -> Result<bool, Failure> {
    let mut diff = Command::new("git");
    diff.args(["diff", "--quiet", base, "--"]).current_dir(root);
    let status = diff
        .status()
        .map_err(|error| Failure::Broken(format!("{diff:?}: {error}")))?;
    let tracked_unchanged = match status.code() {
        Some(0) => true,
        Some(1) => false,
        _ => return Err(Failure::Broken(format!("{diff:?} exited with {status}"))),
    };
    let mut untracked = Command::new("git");
    untracked
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(root);
    Ok(tracked_unchanged && repo::stdout(&mut untracked)?.is_empty())
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
    let status = command
        .status()
        .map_err(|error| Failure::Broken(format!("{command:?}: {error}")))?;
    match status.code() {
        Some(0) => Ok(Vec::new()),
        Some(LINT_GATE_EXIT) => Ok(vec![format!(
            "lint found erosion since {base}: the findings are listed above"
        )]),
        _ => Err(Failure::Broken(format!("{command:?} exited with {status}"))),
    }
}

fn health(root: &Path, health_bin: &Path) -> Result<Figures, Failure> {
    let mut command = Command::new(health_bin);
    command
        .args(["health", "--json", "--repo"])
        .arg(root.join("crates"));
    let output = repo::stdout(&mut command)?;
    let health: Health = serde_json::from_slice(&output)
        .map_err(|error| Failure::Broken(format!("health --json: {error}")))?;
    Ok(health.complexity)
}

fn ratchet(root: &Path, current: Figures, update: bool) -> Result<Vec<String>, Failure> {
    let path = root.join(BASELINE);
    let stored: Option<Figures> = if path.is_file() {
        let text = repo::read(&path)?;
        let figures = serde_json::from_str(&text)
            .map_err(|error| Failure::Broken(format!("{BASELINE}: {error}")))?;
        Some(figures)
    } else {
        None
    };
    if update {
        let lowered = stored.map_or(current, |stored| stored.lowered(current));
        let text = serde_json::to_string_pretty(&lowered)
            .map_err(|error| Failure::Broken(format!("{BASELINE}: {error}")))?;
        repo::write(&path, &format!("{text}\n"))?;
        return Ok(Vec::new());
    }
    Ok(match stored {
        None => vec![format!(
            "{BASELINE} is missing: run cargo xtask erosion with --update"
        )],
        Some(stored) => stored.rises(current),
    })
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
    use super::Figures;

    const STORED: Figures = Figures {
        erosion: 0.2,
        over_threshold: 3,
        clone_share: 0.05,
    };

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
}
