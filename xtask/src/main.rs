//! Development gates for this workspace: each command exits non-zero when its gate fails.

mod erosion;
mod layers;
mod lint_tables;
mod public_api;
mod repo;
mod suppressions;

use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask <command>
  layers                 every crate depends only on crates in lower layers
  suppressions           every allow or expect attribute is listed in crates/suppressions.toml
  lint-tables            crate lint tables and clippy.toml files match the root ones
  public-api [--update]  each crate's public API matches its snapshot
  erosion --lint-bin PATH --health-bin PATH [--base REV] [--update]
                         the pinned agentless-mcp checks pass, and health does not get worse";

pub(crate) enum Failure {
    Usage,
    Gate(Vec<String>),
    Broken(String),
}

pub(crate) fn verdict(problems: Vec<String>) -> Result<(), Failure> {
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Failure::Gate(problems))
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let outcome = repo::root().and_then(|root| match args.split_first() {
        Some((command, flags)) => run(command, flags, &root),
        None => Err(Failure::Usage),
    });
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage) => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        Err(Failure::Gate(problems)) => {
            for problem in &problems {
                eprintln!("{problem}");
            }
            ExitCode::FAILURE
        }
        Err(Failure::Broken(message)) => {
            eprintln!("xtask: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: &str, flags: &[String], root: &Path) -> Result<(), Failure> {
    match (command, flags) {
        ("layers", []) => layers::check(root),
        ("suppressions", []) => suppressions::check(root),
        ("lint-tables", []) => lint_tables::check(root),
        ("public-api", flags) => public_api::check(root, flags),
        ("erosion", flags) => erosion::check(root, flags),
        _ => Err(Failure::Usage),
    }
}
