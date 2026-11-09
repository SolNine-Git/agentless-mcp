//! The parsers of `trace` and `search`, which take the arguments of the tools of the same names.

use app::surface::MORE_SECTIONS;
use prompts::parameter_descriptions as parameter;

use super::{flag, option, repo_flags, subcommand};
use crate::args::{Action, Kind, Parser, TEXT, int_value};

pub(super) fn trace() -> Parser {
    let target = Action {
        strings: &[],
        dest: "target",
        metavar: Some("TARGET"),
        help: Some(parameter::TRACE_TARGET.to_owned()),
        kind: Kind::Positional(app::ops::Arity::OneOrMore),
        required: true,
    };
    let more = Action {
        kind: Kind::Value {
            convert: TEXT,
            choices: &MORE_SECTIONS,
            repeat: false,
        },
        ..option(&["--more"], "more", None, parameter::TRACE_MORE)
    };
    let rev = option(&["--rev"], "rev", Some("REV"), parameter::TRACE_REV);
    let mut actions = vec![crate::args::help_action(), target, more, rev];
    actions.extend(repo_flags());
    subcommand("trace", actions)
}

pub(super) fn search() -> Parser {
    let pattern = Action {
        strings: &[],
        dest: "pattern",
        metavar: Some("PATTERN"),
        help: Some(parameter::SEARCH_PATTERN.to_owned()),
        kind: Kind::Positional(app::ops::Arity::Optional),
        required: false,
    };
    let number = |flags, dest, help: &str| Action {
        kind: Kind::Value {
            convert: int_value,
            choices: &[],
            repeat: false,
        },
        ..option(flags, dest, Some("N"), help)
    };
    let mut actions = vec![
        crate::args::help_action(),
        pattern,
        option(&["--files"], "files", Some("GLOB"), parameter::SEARCH_FILES),
        option(&["--path"], "path", Some("PATH"), parameter::SEARCH_PATH),
        flag(
            &["--ignore-case"],
            "ignore_case",
            parameter::SEARCH_IGNORE_CASE,
        ),
        number(&["--context"], "context", parameter::SEARCH_CONTEXT),
        number(&["--limit"], "limit", parameter::SEARCH_LIMIT),
    ];
    actions.extend(repo_flags());
    subcommand("search", actions)
}
