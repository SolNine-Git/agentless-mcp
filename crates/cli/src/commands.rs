//! The declarations of the command line: the top parser, its subcommands, and the `lint` and
//! `capabilities` parsers with their help texts.

use app::{CHECKS, CheckInput, Severity};
use prompts::messages::LINT_UNKNOWN_SEVERITY;

use crate::args::{Action, Arg, Command, Convert, Group, Kind, Parser, Value};

pub(crate) const PROG: &str = "agentless-mcp";
pub(crate) const CHARS4: &str = "chars4";
pub(crate) const TIKTOKEN: &str = "tiktoken";

const DESCRIPTION: &str = "Model-free tree-sitter repo map, localization and slice machinery.";
const TOKEN_COUNTER_HELP: &str = "how token budgets are estimated (default: chars4). 'tiktoken' \
    needs the 'tokens' extra and shifts every budget";
const NO_AUTO_WARM_HELP: &str = "do not warm cold grammars in the background at startup; \
    grammars then warm only through an explicit warmup (AGENTLESS_MCP_NO_AUTO_WARM in the \
    environment does the same)";
const HELP_HELP: &str = "show this help message and exit";
const REPO_HELP: &str = "repository root (default: the git root enclosing the current directory)";
const JSON_HELP: &str = "emit JSON instead of text";
const NO_CACHE_HELP: &str = "parse on demand and ignore the tag cache for this call";
const LINT_DESCRIPTION: &str = "Run the deterministic patch checks and print what they found. \
    Advisories, warnings and coverage gaps are all reported. By default none of them fails the \
    command: the exit code is 0 unless the invocation itself was unusable. --fail-on makes a \
    finding of a listed severity exit 3, for a CI gate.";
const CANDIDATES_HELP: &str =
    "a patch file, or a directory of them (edits.json or SEARCH/REPLACE text)";
const DIFF_HELP: &str = "a unified diff to check instead (git diff, or a format-patch body); the \
    checks compare it against --repo as it stands, so --repo must be a checkout of the diff's \
    BASE, not a tree with the diff already applied";
const AFTER_HELP: &str = "the file that holds the proposed whole new text of --path";

const TEXT: Convert = |arg| Ok(Value::Text(arg.clone()));

pub(crate) static COMMANDS: [Command; 24] = [
    unported("map", "ranked, token-budgeted repository map"),
    unported("tree", "gitignore-aware directory tree"),
    unported("skeleton", "signatures with bodies elided"),
    unported("expand", "full bodies for named stable ids"),
    unported("slice", "numbered lines with scope headers"),
    unported("find-symbol", "substring or qualified symbol lookup"),
    unported("refs", "fan-in: what references this symbol"),
    unported(
        "history",
        "why does this code exist: the commits that touched a symbol's lines",
    ),
    unported("explain", "one symbol card: definition, fan-out, fan-in"),
    unported("path", "shortest resolved path between two symbols"),
    unported("cycles", "module-level import cycles"),
    unported("communities", "which files belong together"),
    unported("diagram", "mermaid flowchart of the module graph"),
    unported(
        "health",
        "orphan candidates, unused exports, hubs and complexity hotspots",
    ),
    unported("html", "interactive HTML export of the module graph"),
    unported(
        "resolve-locs",
        "turn class:/function:/line: strings into intervals",
    ),
    unported("patch", "SEARCH/REPLACE patch machinery (write side)"),
    Command {
        name: "lint",
        help: "deterministic hallucination checks over candidate patches (write side)",
        parser: Some(lint),
    },
    unported("validate", "run candidate patches against the tests"),
    unported("vote", "rank validated candidates by equivalence cluster"),
    unported("warmup", "fetch and probe grammars; fails loudly"),
    unported("index", "build or refresh the tag cache"),
    Command {
        name: "capabilities",
        help: "grammars, versions and caps in force",
        parser: Some(capabilities),
    },
    unported("guide", "print the packaged agent usage guide"),
];

const fn unported(name: &'static str, help: &'static str) -> Command {
    Command {
        name,
        help,
        parser: None,
    }
}

pub(crate) fn top() -> Parser {
    let counter = Action {
        strings: &["--token-counter"],
        dest: "token_counter",
        metavar: None,
        help: Some(TOKEN_COUNTER_HELP.to_owned()),
        kind: Kind::Value {
            convert: TEXT,
            choices: &[CHARS4, TIKTOKEN],
        },
    };
    let commands = Action {
        strings: &[],
        dest: "command",
        metavar: None,
        help: None,
        kind: Kind::Commands(&COMMANDS),
    };
    let actions = vec![
        help(),
        counter,
        flag(&["--no-auto-warm"], "no_auto_warm", NO_AUTO_WARM_HELP),
        commands,
    ];
    Parser {
        prog: PROG,
        description: Some(DESCRIPTION),
        actions,
        group: None,
    }
}

fn lint() -> Parser {
    let mut actions = vec![
        help(),
        value(
            &["--candidates"],
            "candidates",
            "PATH",
            CANDIDATES_HELP.to_owned(),
        ),
        value(&["--diff"], "diff", "FILE", DIFF_HELP.to_owned()),
        value(&["--path"], "path", "REL", path_help()),
        value(&["--since"], "since", "REV", since_help()),
        value(&["--after"], "after", "FILE", AFTER_HELP.to_owned()),
    ];
    actions.push(Action {
        kind: Kind::Value {
            convert: check_names,
            choices: &[],
        },
        ..value(&["--only"], "only", "CHECK[,CHECK...]", only_help())
    });
    actions.push(Action {
        kind: Kind::Value {
            convert: severities,
            choices: &[],
        },
        ..value(
            &["--fail-on"],
            "fail_on",
            "SEVERITY[,SEVERITY...]",
            fail_on_help(),
        )
    });
    actions.extend(repo_flags());
    Parser {
        prog: "agentless-mcp lint",
        description: Some(LINT_DESCRIPTION),
        actions,
        group: Some(Group {
            members: 1..5,
            required: true,
        }),
    }
}

fn capabilities() -> Parser {
    let mut actions = vec![help()];
    actions.extend(repo_flags());
    Parser {
        prog: "agentless-mcp capabilities",
        description: None,
        actions,
        group: None,
    }
}

fn repo_flags() -> [Action; 3] {
    [
        value(&["--repo"], "repo", "PATH", REPO_HELP.to_owned()),
        flag(&["--json"], "json", JSON_HELP),
        flag(&["--no-cache"], "no_cache", NO_CACHE_HELP),
    ]
}

fn help() -> Action {
    Action {
        strings: &["-h", "--help"],
        dest: "help",
        metavar: None,
        help: Some(HELP_HELP.to_owned()),
        kind: Kind::Help,
    }
}

fn flag(strings: &'static [&'static str], dest: &'static str, help: &str) -> Action {
    Action {
        strings,
        dest,
        metavar: None,
        help: Some(help.to_owned()),
        kind: Kind::Flag,
    }
}

fn value(
    strings: &'static [&'static str],
    dest: &'static str,
    metavar: &'static str,
    help: String,
) -> Action {
    Action {
        strings,
        dest,
        metavar: Some(metavar),
        help: Some(help),
        kind: Kind::Value {
            convert: TEXT,
            choices: &[],
        },
    }
}

fn path_help() -> String {
    format!(
        "a file in --repo whose proposed whole new text is in --after (the per-edit hook form). \
         Runs {}, which read only this file. --only can add clone_delta, which reads the clone \
         blocks of every tier-1 source file from the tag cache only, and gives one not_checked \
         row when there is no cache",
        edited_checks()
    )
}

fn since_help() -> String {
    format!(
        "compare each file changed since the commit REV with its working-tree text (review mode). \
         Runs {}, which read only the changed files. --only can add clone_delta, which also reads \
         every tier-1 source file. For a branch, pass --since \"$(git merge-base HEAD main)\"",
        edited_checks()
    )
}

fn only_help() -> String {
    let all: Vec<&str> = CHECKS.iter().map(|check| check.name).collect();
    let pairs = CHECKS
        .iter()
        .filter(|check| check.input != CheckInput::Patch);
    let pairs: Vec<&str> = pairs.map(|check| check.name).collect();
    format!(
        "run only these checks: {}. The pair checks ({}) compare whole file texts. {} read only \
         the edited files. clone_delta also reads every tier-1 source file outside test and \
         fixture paths. A run with only pair checks skips the symbol scan that the other checks \
         need",
        all.join(", "),
        pairs.join(", "),
        edited_checks()
    )
}

fn fail_on_help() -> String {
    format!(
        "exit 3 when a finding has one of these severities: {}. Without it, lint exits 0 on a \
         run that happened",
        severity_names()
    )
}

// The checks that read only the edited files: the default of the two pair forms.
fn edited_checks() -> String {
    let edited = CHECKS
        .iter()
        .filter(|check| check.input == CheckInput::Pairs);
    edited
        .map(|check| check.name)
        .collect::<Vec<_>>()
        .join(" and ")
}

fn severity_names() -> String {
    Severity::ALL.map(Severity::as_str).join(", ")
}

// The `--only` value: names split at commas, trimmed, each kept once in its first place.
fn check_names(arg: &Arg) -> Result<Value, String> {
    let mut names: Vec<String> = Vec::new();
    for name in arg.text.split(',').map(pycompat::strip) {
        if !names.iter().any(|kept| kept == name) {
            names.push(name.to_owned());
        }
    }
    match app::unknown_check_refusal(&names) {
        Some(refusal) => Err(refusal),
        None => Ok(Value::Names(names)),
    }
}

// The `--fail-on` value: severity names split at commas and trimmed; the first unknown refuses.
fn severities(arg: &Arg) -> Result<Value, String> {
    let names: Vec<String> = arg
        .text
        .split(',')
        .map(|n| pycompat::strip(n).to_owned())
        .collect();
    let known = |name: &String| Severity::ALL.iter().any(|s| s.as_str() == name.as_str());
    match names.iter().find(|name| !known(name)) {
        Some(unknown) => Err(LINT_UNKNOWN_SEVERITY.render((unknown, &severity_names()))),
        None => Ok(Value::Names(names)),
    }
}
