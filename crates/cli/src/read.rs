//! The read subcommands: the shared steps (the repository, its session, the answer in either form)
//! and the step of each read operation that this build runs.

mod body;
mod cycles;
mod diagram;
mod graph;
mod history;
mod html;
mod index;
mod map;
mod refs;
mod symbols;
mod tools;
mod views;

use std::path::Path;

use app::ops::{Op, Value as Declared};
use app::{IntArg, Services, Session};
use render::{Receipt, View};

use crate::Streams;
use crate::args::{Arg, Namespace, Value, float_text};
use crate::exit::{EXIT_DOMAIN, EXIT_OK, EXIT_USAGE, code_for, fail, write_answer};
use crate::invocation::{ReadCall, RepoFlags, repo_flags};
use crate::steps::{answer, opened};

// The step of one read subcommand.
type Step = fn(&ReadArgs<'_>, &Services, &mut Streams<'_>) -> u8;

// The read subcommands that this build runs. Each one parses before it runs, so a subcommand
// missing here still answers its help and its usage errors.
const STEPS: [(&str, Step); 19] = [
    ("map", map::map),
    ("tree", views::tree),
    ("skeleton", views::skeleton),
    ("expand", symbols::expand),
    ("slice", views::slice),
    ("find-symbol", symbols::find),
    ("refs", refs::refs),
    ("history", history::history),
    ("explain", graph::explain),
    ("path", graph::path),
    ("cycles", cycles::cycles),
    ("communities", graph::communities),
    ("diagram", diagram::diagram),
    ("health", graph::health),
    ("html", html::html),
    ("resolve-locs", views::resolve_locs),
    ("warmup", tools::warmup),
    ("index", index::index),
    ("guide", tools::guide),
];

pub(crate) fn run(call: &ReadCall, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let name = call.op.name;
    let Some((_, step)) = STEPS.iter().find(|(known, _)| *known == name) else {
        let message =
            format!("this build does not run {name} yet. Run it with the Python agentless-mcp.");
        return fail(streams.err, &message, EXIT_USAGE);
    };
    let args = ReadArgs {
        op: call.op,
        namespace: &call.namespace,
    };
    step(&args, services, streams)
}

// The values of one read subcommand's parse, with the declared default of each value it left
// unset.
pub(crate) struct ReadArgs<'a> {
    op: &'static Op,
    namespace: &'a Namespace,
}

impl ReadArgs<'_> {
    pub(crate) fn flags(&self) -> RepoFlags {
        repo_flags(self.namespace)
    }

    // An integer parameter: the value given, else the declared default.
    pub(crate) fn int(&self, dest: &str) -> IntArg {
        if let Some(value) = self.given_int(dest) {
            return value;
        }
        let declared = self.op.params.iter().find(|param| param.dest == dest);
        match declared.map(|param| param.value) {
            Some(Declared::Int(default)) => IntArg::from_usize(default),
            _ => panic!("{} declares no integer {dest} with a default", self.op.name),
        }
    }

    // An integer parameter that is unset unless given, for the config or the operation to decide.
    pub(crate) fn given_int(&self, dest: &str) -> Option<IntArg> {
        match self.namespace.get(dest) {
            Some(Value::Int(value)) => Some(value.clone()),
            _ => None,
        }
    }

    pub(crate) fn text(&self, dest: &str) -> Option<&Arg> {
        self.namespace.text(dest)
    }

    pub(crate) fn list(&self, dest: &str) -> &[Arg] {
        self.namespace.list(dest)
    }

    pub(crate) fn flag(&self, dest: &str) -> bool {
        self.namespace.flag(dest)
    }

    // A switch that is true when given and unset, for the config to decide, when not.
    pub(crate) fn switch(&self, dest: &str) -> Option<bool> {
        self.flag(dest).then_some(true)
    }

    // A real-number parameter, unset unless given. The parse already checked its text.
    pub(crate) fn float(&self, dest: &str) -> Option<f64> {
        let text = self.text(dest)?;
        Some(float_text(&text.text).expect("the parser accepts only a real number"))
    }
}

// The text of a path argument, or the refusal of one that is not UTF-8 (ledger deviation).
pub(crate) fn utf8_path(arg: &Arg, name: &str, streams: &mut Streams<'_>) -> Result<String, u8> {
    let message = || format!("{name} is not valid UTF-8: {}", arg.text);
    let text = arg.raw.to_str().map(str::to_owned);
    text.ok_or_else(|| fail(streams.err, &message(), EXIT_USAGE))
}

// Resolves the repository a read subcommand is about, which is the enclosing git root unless
// --repo names one, then opens its session with the fact source.
pub(crate) fn open_reading(
    flags: &RepoFlags,
    services: &Services,
    streams: &mut Streams<'_>,
) -> Result<Session, u8> {
    let repo = flags.repo.as_deref().map(Path::new);
    let repo = app::resolve_in_git(services, repo)
        .map_err(|error| fail(streams.err, &error.to_string(), code_for(&error)))?;
    opened(streams, Session::open_facts(services, repo, flags.no_cache))
}

// Prints the operation's warnings, then its answer in the form --json asks for, or its failure.
pub(crate) fn emit<V: View>(
    outcome: app::Noted<Result<V, app::Error>>,
    session: &Session,
    services: &Services,
    flags: &RepoFlags,
    streams: &mut Streams<'_>,
) -> u8 {
    let answering = Answering {
        session,
        services,
        flags,
    };
    emit_then(outcome, &answering, streams, |_| (String::new(), EXIT_OK))
}

// What an answer is written under: the session, the services and the repository flags.
pub(crate) struct Answering<'a> {
    pub(crate) session: &'a Session,
    pub(crate) services: &'a Services,
    pub(crate) flags: &'a RepoFlags,
}

// As `emit`, with the stderr text and the exit code that `finish` reads off the view; that text
// follows the answer.
pub(crate) fn emit_then<V: View>(
    outcome: app::Noted<Result<V, app::Error>>,
    answering: &Answering<'_>,
    streams: &mut Streams<'_>,
    finish: impl FnOnce(&V) -> (String, u8),
) -> u8 {
    crate::steps::diagnostics(streams, &outcome.diagnostics);
    let view = match outcome.value {
        Ok(view) => view,
        Err(error) => return fail(streams.err, &error.to_string(), code_for(&error)),
    };
    let counter = &*answering.services.counter;
    let receipt = Receipt::of(answering.session).counted_by(counter);
    let rendered = if answering.flags.json {
        render::json(&receipt, &view)
    } else {
        Ok(render::text(&receipt, &view))
    };
    let (err, code) = finish(&view);
    match rendered {
        Ok(text) => write_answer(streams, &text, &err, code),
        Err(error) => answer(streams, Err(error), EXIT_DOMAIN),
    }
}
