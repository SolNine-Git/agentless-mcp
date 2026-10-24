//! The typed command that a parsed command line asks for.

use std::ffi::OsString;

use app::ops::Op;
use app::{
    ApplyMode, DEFAULT_JOBS, DEFAULT_REPEAT_BASELINE, DEFAULT_TIMEOUT_SECONDS, IntArg, Severity,
};

use crate::args::{Arg, Chosen, Namespace, Parsed, Value};
use crate::commands::TIKTOKEN;

pub(crate) struct Invocation {
    pub(crate) tiktoken: bool,
    pub(crate) command: Command,
}

pub(crate) enum Command {
    Lint(Lint),
    Capabilities(RepoFlags),
    Read(ReadCall),
    PatchParse(PatchInput),
    PatchEdit(PatchEdit),
    Validate(Validate),
    Vote(Vote),
}

// A read subcommand: its operation's declaration and the values its parser set.
pub(crate) struct ReadCall {
    pub(crate) op: &'static Op,
    pub(crate) namespace: Namespace,
}

pub(crate) struct RepoFlags {
    pub(crate) repo: Option<OsString>,
    pub(crate) json: bool,
    pub(crate) no_cache: bool,
}

pub(crate) struct Lint {
    pub(crate) source: Source,
    pub(crate) after: Option<Arg>,
    pub(crate) only: Option<Vec<String>>,
    pub(crate) fail_on: Vec<Severity>,
    pub(crate) flags: RepoFlags,
}

pub(crate) enum Source {
    Candidates(Arg),
    Diff(Arg),
    Path(Arg),
    Since(Arg),
}

// Where a patch step reads its patch: the file `-f` names, or stdin without it or for `-`.
pub(crate) struct PatchInput {
    pub(crate) file: Option<Arg>,
}

// A patch step that reads the repository: check, apply or normalize.
pub(crate) struct PatchEdit {
    pub(crate) step: EditStep,
    pub(crate) input: PatchInput,
    pub(crate) flags: RepoFlags,
}

#[derive(Clone, Copy)]
pub(crate) enum EditStep {
    Check,
    Apply(ApplyMode),
    Normalize,
}

// The values of a validate command line, each number as written, with the defaults filled in.
pub(crate) struct Validate {
    pub(crate) candidates: Arg,
    pub(crate) repo: Option<OsString>,
    pub(crate) test_cmd: Option<Arg>,
    pub(crate) repro_cmd: Option<Arg>,
    pub(crate) allow_repo_test_cmd: bool,
    pub(crate) allow_test_config_edits: bool,
    pub(crate) pass_env: Vec<String>,
    pub(crate) timeout: IntArg,
    pub(crate) jobs: IntArg,
    pub(crate) run_timeout: Option<IntArg>,
    pub(crate) repeat_baseline: IntArg,
    pub(crate) output: Option<Arg>,
}

pub(crate) struct Vote {
    pub(crate) verdicts: Arg,
    pub(crate) json: bool,
}

impl Invocation {
    pub(crate) fn new(parsed: Parsed) -> Invocation {
        let counter = parsed.top.text("token_counter");
        let command = match parsed.chosen {
            Chosen::Ported { path, namespace } => ported(&path, &namespace),
            Chosen::Read { op, namespace } => Command::Read(ReadCall { op, namespace }),
        };
        Invocation {
            tiktoken: counter.is_some_and(|arg| arg.text == TIKTOKEN),
            command,
        }
    }
}

// The command of a hand-built parser path. The paths are the ones that commands.rs declares.
fn ported(path: &[&str], namespace: &Namespace) -> Command {
    match path {
        ["lint"] => Command::Lint(lint(namespace)),
        ["capabilities"] => Command::Capabilities(repo_flags(namespace)),
        ["patch", "parse"] => Command::PatchParse(patch_input(namespace)),
        ["patch", step] => Command::PatchEdit(PatchEdit {
            step: edit_step(step, namespace),
            input: patch_input(namespace),
            flags: repo_flags(namespace),
        }),
        ["validate"] => Command::Validate(validate(namespace)),
        ["vote"] => Command::Vote(Vote {
            verdicts: required(namespace, "verdicts"),
            json: namespace.flag("json"),
        }),
        _ => panic!("commands.rs declares no command {path:?}"),
    }
}

fn edit_step(step: &str, namespace: &Namespace) -> EditStep {
    match step {
        "check" => EditStep::Check,
        "apply" if namespace.flag("in_place") => EditStep::Apply(ApplyMode::InPlace),
        "apply" => EditStep::Apply(ApplyMode::Worktree),
        "normalize" => EditStep::Normalize,
        _ => panic!("commands.rs declares no patch step {step}"),
    }
}

fn patch_input(namespace: &Namespace) -> PatchInput {
    PatchInput {
        file: namespace.text("file").cloned(),
    }
}

fn lint(namespace: &Namespace) -> Lint {
    let fail_on = namespace.names("fail_on").unwrap_or_default();
    Lint {
        source: source(namespace),
        after: namespace.text("after").cloned(),
        only: namespace.names("only").map(<[String]>::to_vec),
        fail_on: Severity::ALL
            .into_iter()
            .filter(|severity| fail_on.iter().any(|name| name == severity.as_str()))
            .collect(),
        flags: repo_flags(namespace),
    }
}

// The parser's required group sets exactly one of the four sources.
fn source(namespace: &Namespace) -> Source {
    let given = |dest: &str| namespace.text(dest).cloned();
    (given("path").map(Source::Path))
        .or_else(|| given("since").map(Source::Since))
        .or_else(|| given("diff").map(Source::Diff))
        .unwrap_or_else(|| Source::Candidates(required(namespace, "candidates")))
}

fn validate(namespace: &Namespace) -> Validate {
    let int = |dest: &str| match namespace.get(dest) {
        Some(Value::Int(value)) => Some(value.clone()),
        _ => None,
    };
    let given_or = |dest: &str, default: usize| int(dest).unwrap_or(IntArg::from_usize(default));
    Validate {
        candidates: required(namespace, "candidates"),
        repo: namespace.text("repo").map(|arg| arg.raw.clone()),
        test_cmd: namespace.text("test_cmd").cloned(),
        repro_cmd: namespace.text("repro_cmd").cloned(),
        allow_repo_test_cmd: namespace.flag("allow_repo_test_cmd"),
        allow_test_config_edits: namespace.flag("allow_test_config_edits"),
        pass_env: (namespace.list("pass_env").iter())
            .map(|arg| arg.text.clone())
            .collect(),
        timeout: given_or("timeout", DEFAULT_TIMEOUT_SECONDS),
        jobs: given_or("jobs", DEFAULT_JOBS),
        run_timeout: int("run_timeout"),
        repeat_baseline: given_or("repeat_baseline", DEFAULT_REPEAT_BASELINE),
        output: namespace.text("output").cloned(),
    }
}

// The value of an option that the parser requires, so the parse set it.
fn required(namespace: &Namespace, dest: &str) -> Arg {
    let value = namespace.text(dest).cloned();
    value.unwrap_or_else(|| panic!("the parser requires {dest}"))
}

pub(crate) fn repo_flags(namespace: &Namespace) -> RepoFlags {
    RepoFlags {
        repo: namespace.text("repo").map(|arg| arg.raw.clone()),
        json: namespace.flag("json"),
        no_cache: namespace.flag("no_cache"),
    }
}
