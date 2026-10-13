//! The declarations of the command line: the top parser, its subcommands, and the `lint` and
//! `capabilities` parsers with their help texts. The read subcommands' parsers come from
//! `app::ops::OPS`.

use app::ops::{JSON, NO_CACHE, OPS, Op, REPO};
use app::{CHECKS, CheckInput, Severity};
use prompts::messages::LINT_UNKNOWN_SEVERITY;

use crate::args::{
    Action, Arg, Command, Convert, Form, Group, Kind, Parser, TEXT, Value, action, help_action,
};

pub(crate) const PROG: &str = "agentless-mcp";
pub(crate) const CHARS4: &str = "chars4";
pub(crate) const TIKTOKEN: &str = "tiktoken";

const DESCRIPTION: &str = "Model-free tree-sitter repo map, localization and slice machinery.";
const TOKEN_COUNTER_HELP: &str = "how token budgets are estimated (default: chars4). 'tiktoken' \
    needs the 'tokens' extra and shifts every budget";
const NO_AUTO_WARM_HELP: &str = "do not warm cold grammars in the background at startup; \
    grammars then warm only through an explicit warmup (AGENTLESS_MCP_NO_AUTO_WARM in the \
    environment does the same)";
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

// The subcommands in the reference order. A read subcommand names its entry of OPS.
pub(crate) static COMMANDS: [Command; 24] = [
    read(&OPS[0]),
    read(&OPS[1]),
    read(&OPS[2]),
    read(&OPS[3]),
    read(&OPS[4]),
    read(&OPS[5]),
    read(&OPS[6]),
    read(&OPS[7]),
    read(&OPS[8]),
    read(&OPS[9]),
    read(&OPS[10]),
    read(&OPS[11]),
    read(&OPS[12]),
    read(&OPS[13]),
    read(&OPS[14]),
    read(&OPS[15]),
    unported("patch", "SEARCH/REPLACE patch machinery (write side)"),
    Command {
        name: "lint",
        help: "deterministic hallucination checks over candidate patches (write side)",
        form: Form::Built(lint),
    },
    unported("validate", "run candidate patches against the tests"),
    unported("vote", "rank validated candidates by equivalence cluster"),
    read(&OPS[16]),
    read(&OPS[17]),
    Command {
        name: "capabilities",
        help: "grammars, versions and caps in force",
        form: Form::Built(capabilities),
    },
    read(&OPS[18]),
];

const fn unported(name: &'static str, help: &'static str) -> Command {
    Command {
        name,
        help,
        form: Form::Unported,
    }
}

const fn read(op: &'static Op) -> Command {
    Command {
        name: op.name,
        help: op.summary,
        form: Form::Read(op),
    }
}

pub(crate) fn top() -> Parser {
    let counter = Action {
        kind: Kind::Value {
            convert: TEXT,
            choices: &[CHARS4, TIKTOKEN],
            repeat: false,
        },
        ..option(
            &["--token-counter"],
            "token_counter",
            None,
            TOKEN_COUNTER_HELP,
        )
    };
    let commands = Action {
        strings: &[],
        dest: "command",
        metavar: None,
        help: None,
        kind: Kind::Commands(&COMMANDS),
        required: true,
    };
    let actions = vec![
        help_action(),
        counter,
        flag(&["--no-auto-warm"], "no_auto_warm", NO_AUTO_WARM_HELP),
        commands,
    ];
    Parser {
        prog: PROG.to_owned(),
        description: Some(DESCRIPTION),
        actions,
        group: None,
    }
}

fn lint() -> Parser {
    let mut actions = vec![
        help_action(),
        option(
            &["--candidates"],
            "candidates",
            Some("PATH"),
            CANDIDATES_HELP,
        ),
        option(&["--diff"], "diff", Some("FILE"), DIFF_HELP),
        option(&["--path"], "path", Some("REL"), &path_help()),
        option(&["--since"], "since", Some("REV"), &since_help()),
        option(&["--after"], "after", Some("FILE"), AFTER_HELP),
    ];
    actions.push(Action {
        kind: converted(check_names),
        ..option(&["--only"], "only", Some("CHECK[,CHECK...]"), &only_help())
    });
    actions.push(Action {
        kind: converted(severities),
        ..option(
            &["--fail-on"],
            "fail_on",
            Some("SEVERITY[,SEVERITY...]"),
            &fail_on_help(),
        )
    });
    actions.extend(repo_flags());
    Parser {
        prog: format!("{PROG} lint"),
        description: Some(LINT_DESCRIPTION),
        actions,
        group: Some(Group {
            members: 1..5,
            required: true,
        }),
    }
}

fn capabilities() -> Parser {
    let mut actions = vec![help_action()];
    actions.extend(repo_flags());
    Parser {
        prog: format!("{PROG} capabilities"),
        description: None,
        actions,
        group: None,
    }
}

// The repository options, as the read operations declare them.
fn repo_flags() -> impl Iterator<Item = Action> {
    [&REPO, &JSON, &NO_CACHE].into_iter().filter_map(action)
}

fn flag(strings: &'static [&'static str], dest: &'static str, help: &str) -> Action {
    Action {
        kind: Kind::Flag,
        ..option(strings, dest, None, help)
    }
}

fn option(
    strings: &'static [&'static str],
    dest: &'static str,
    metavar: Option<&'static str>,
    help: &str,
) -> Action {
    Action {
        strings,
        dest,
        metavar,
        help: Some(help.to_owned()),
        kind: converted(TEXT),
        required: false,
    }
}

const fn converted(convert: Convert) -> Kind {
    Kind::Value {
        convert,
        choices: &[],
        repeat: false,
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
    for name in arg.text.split(',').map(text::strip) {
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
        .map(|n| text::strip(n).to_owned())
        .collect();
    let known = |name: &String| Severity::ALL.iter().any(|s| s.as_str() == name.as_str());
    match names.iter().find(|name| !known(name)) {
        Some(unknown) => Err(LINT_UNKNOWN_SEVERITY.render((unknown, &severity_names()))),
        None => Ok(Value::Names(names)),
    }
}

#[cfg(test)]
mod tests;
