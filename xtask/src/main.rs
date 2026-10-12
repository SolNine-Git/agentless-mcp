//! Development gates for this workspace: each command exits non-zero when its gate fails.

mod erosion;
mod generate;
mod ground_truth;
mod layers;
mod lint_tables;
mod policy;
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

type Gate = fn(&Path) -> Result<(), Failure>;
type FlagGate = fn(&Path, &[String]) -> Result<(), Failure>;

enum Handler {
    Plain(Gate),
    Flags(FlagGate),
    Print(&'static str),
}

struct Task {
    name: &'static str,
    usage: &'static str,
    handler: Handler,
}

const TASKS: [Task; 10] = [
    Task {
        name: "static-gates",
        usage: "  static-gates           run layers, suppressions and lint-tables, and report all three",
        handler: Handler::Plain(static_gates),
    },
    Task {
        name: "layers",
        usage: "  layers                 every crate depends only on crates in lower layers, and
                         crates/dependencies.toml lists each direct third-party dependency",
        handler: Handler::Plain(layers::check),
    },
    Task {
        name: "suppressions",
        usage: "  suppressions           every allow, expect, warn, ignore, clippy:: or rustfmt:: attribute is
                         listed in crates/suppressions.toml",
        handler: Handler::Plain(suppressions::check),
    },
    Task {
        name: "lint-tables",
        usage: "  lint-tables            lint tables and tool config files match the root ones, and unsafe
                         stays in crates/syntax/src/ffi.rs",
        handler: Handler::Plain(lint_tables::check),
    },
    Task {
        name: "ground-truth",
        usage: "  ground-truth           clippy with each warn and deny lint force-warned finds only the
                         suppressions that crates/suppressions.toml lists, rustc reads only
                         scanned sources, an unknown cfg fails the build,
                         crates/dependencies.toml lists each build-time package, and the
                         ledger counts each ignored test",
        handler: Handler::Plain(ground_truth::check),
    },
    Task {
        name: "public-api",
        usage: "  public-api [--update]  each library's public API matches its snapshot",
        handler: Handler::Flags(public_api::check),
    },
    Task {
        name: "erosion",
        usage: "  erosion --lint-bin PATH --health-bin PATH [--base REV] [--update]
                         the pinned agentless-mcp checks pass, and health does not get worse",
        handler: Handler::Flags(erosion::check),
    },
    Task {
        name: "policy",
        usage: "  policy [--base REV]    each policy file at HEAD and in the working tree equals its reference
                         (its version at the base, or at the one commit that created it), or each
                         commit that changed it carries a Policy-Change: <path> trailer and one of
                         those commits made HEAD's value",
        handler: Handler::Flags(policy::check),
    },
    Task {
        name: "generate",
        usage: "  generate               write the generated Rust of the prompts and syntax crates",
        handler: Handler::Plain(generate::run),
    },
    Task {
        name: "nightly",
        usage: "  nightly                print the nightly toolchain that public-api needs",
        handler: Handler::Print(public_api::TOOLCHAIN),
    },
];

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
    let task = TASKS
        .iter()
        .find(|task| task.name == command)
        .ok_or(Failure::Usage)?;
    match (&task.handler, flags) {
        (Handler::Plain(gate), []) => gate(root),
        (Handler::Flags(gate), flags) => gate(root, flags),
        (Handler::Print(text), []) => {
            println!("{text}");
            Ok(())
        }
        (Handler::Plain(_) | Handler::Print(_), _) => Err(Failure::Usage),
    }
}

fn usage() -> String {
    let lines: Vec<&str> = TASKS.iter().map(|task| task.usage).collect();
    format!("usage: cargo xtask <command>\n{}", lines.join("\n"))
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
        Failure::Usage => eprintln!("{}", usage()),
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
