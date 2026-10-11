//! Development gates for this workspace: each command exits non-zero when its gate fails.

mod erosion;
mod layers;
mod lint_tables;
mod process;
mod public_api;
mod repo;
mod sources;
mod suppressions;
mod tokens;
mod tool_config;
mod workspace;

use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask <command>
  static-gates           run layers, suppressions and lint-tables, and report all three
  layers                 every crate depends only on crates in lower layers
  suppressions           every allow, expect, warn, clippy:: or rustfmt:: attribute is listed
                         in crates/suppressions.toml
  lint-tables            lint tables and tool config files match the root ones, and unsafe
                         stays in crates/syntax/src/ffi.rs
  public-api [--update]  each library's public API matches its snapshot
  erosion --lint-bin PATH --health-bin PATH [--base REV] [--update]
                         the pinned agentless-mcp checks pass, and health does not get worse
  nightly                print the nightly toolchain that public-api needs";

type Gate = fn(&Path) -> Result<(), Failure>;

const STATIC_GATES: [(&str, Gate); 3] = [
    ("layers", layers::check),
    ("suppressions", suppressions::check),
    ("lint-tables", lint_tables::check),
];

#[derive(Debug)]
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
        Err(failure) => {
            report(&failure);
            exit_code(&failure)
        }
    }
}

fn run(command: &str, flags: &[String], root: &Path) -> Result<(), Failure> {
    match (command, flags) {
        ("static-gates", []) => static_gates(root),
        ("layers", []) => layers::check(root),
        ("suppressions", []) => suppressions::check(root),
        ("lint-tables", []) => lint_tables::check(root),
        ("public-api", flags) => public_api::check(root, flags),
        ("erosion", flags) => erosion::check(root, flags),
        ("nightly", []) => {
            println!("{}", public_api::TOOLCHAIN);
            Ok(())
        }
        _ => Err(Failure::Usage),
    }
}

fn static_gates(root: &Path) -> Result<(), Failure> {
    let mut failed = Vec::new();
    for (name, gate) in STATIC_GATES {
        match gate(root) {
            Ok(()) => eprintln!("xtask static-gates: {name} passed"),
            Err(failure) => {
                eprintln!("xtask static-gates: {name} failed");
                report(&failure);
                failed.push(name);
            }
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        verdict(vec![format!(
            "xtask static-gates: {} failed",
            failed.join(", ")
        )])
    }
}

fn report(failure: &Failure) {
    match failure {
        Failure::Usage => eprintln!("{USAGE}"),
        Failure::Gate(problems) => {
            for problem in problems {
                eprintln!("{problem}");
            }
        }
        Failure::Broken(message) => eprintln!("xtask: {message}"),
    }
}

fn exit_code(failure: &Failure) -> ExitCode {
    match failure {
        Failure::Usage => ExitCode::from(2),
        Failure::Gate(_) | Failure::Broken(_) => ExitCode::FAILURE,
    }
}
