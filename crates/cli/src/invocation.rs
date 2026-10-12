//! The typed command that a parsed command line asks for.

use std::ffi::OsString;

use app::Severity;

use crate::args::{Arg, Chosen, Namespace, Parsed};
use crate::commands::TIKTOKEN;

pub(crate) struct Invocation {
    pub(crate) tiktoken: bool,
    pub(crate) command: Command,
}

pub(crate) enum Command {
    Lint(Lint),
    Capabilities(RepoFlags),
    Unported(&'static str),
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
    Candidates,
    Diff,
    Path(Arg),
    Since(Arg),
}

impl Invocation {
    pub(crate) fn new(parsed: Parsed) -> Invocation {
        let counter = parsed.top.text("token_counter");
        let command = match parsed.chosen {
            Chosen::Ported {
                name: "lint",
                namespace,
            } => Command::Lint(lint(&namespace)),
            Chosen::Ported {
                name: "capabilities",
                namespace,
            } => Command::Capabilities(repo_flags(&namespace)),
            Chosen::Unported(name) | Chosen::Ported { name, .. } => Command::Unported(name),
        };
        Invocation {
            tiktoken: counter.is_some_and(|arg| arg.text == TIKTOKEN),
            command,
        }
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
    if let Some(path) = namespace.text("path") {
        return Source::Path(path.clone());
    }
    if let Some(revision) = namespace.text("since") {
        return Source::Since(revision.clone());
    }
    if namespace.text("diff").is_some() {
        return Source::Diff;
    }
    Source::Candidates
}

fn repo_flags(namespace: &Namespace) -> RepoFlags {
    RepoFlags {
        repo: namespace.text("repo").map(|arg| arg.raw.clone()),
        json: namespace.flag("json"),
        no_cache: namespace.flag("no_cache"),
    }
}
