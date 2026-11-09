//! The options of hook install, hook status and hook uninstall: `--name value` or
//! `--name=value`, and the `--dry-run` flag.

use std::ffi::OsString;

use prompts::messages::{
    ACCEPTS_CHOICE, ACCEPTS_SERVER_NAME, ACCEPTS_TEXT, ARGUMENT_INVALID, ARGUMENT_MISSING,
    ARGUMENT_UNKNOWN, LISTED_OR,
};

const SCOPE: &str = "--scope";
const REPO: &str = "--repo";
const SERVER_NAME: &str = "--server-name";
const DRY_RUN: &str = "--dry-run";
const HELP: [&str; 2] = ["-h", "--help"];

// The settings file that a command writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    User,
    Project,
    Local,
}

const SCOPES: [(&str, Scope); 3] = [
    ("user", Scope::User),
    ("project", Scope::Project),
    ("local", Scope::Local),
];

// The options that each command takes, and its help text.
#[derive(Clone, Copy)]
pub(crate) struct Takes {
    pub(crate) command: &'static str,
    pub(crate) help: &'static str,
    pub(crate) scope: bool,
    pub(crate) server_name: bool,
    pub(crate) dry_run: bool,
}

// Why the words give no options to run with: the refusal of a word, or a request for the help.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Stop {
    Refused(String),
    Help,
}

impl From<String> for Stop {
    fn from(message: String) -> Self {
        Stop::Refused(message)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Options {
    pub(crate) scope: Scope,
    pub(crate) repo: Option<String>,
    pub(crate) server_name: Option<String>,
    pub(crate) dry_run: bool,
}

// The options, or the stop at the first word that is a help flag or not an option.
pub(crate) fn parsed(args: &[OsString], takes: Takes) -> Result<Options, Stop> {
    let mut options = Options {
        scope: Scope::User,
        repo: None,
        server_name: None,
        dry_run: false,
    };
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let word = word.to_string_lossy();
        if HELP.contains(&&*word) {
            return Err(Stop::Help);
        }
        let (name, attached) = word
            .split_once('=')
            .map_or((&*word, None), |(name, value)| {
                (name, Some(value.to_owned()))
            });
        if name == DRY_RUN && takes.dry_run && attached.is_none() {
            options.dry_run = true;
            continue;
        }
        if !takes_value(name, takes) {
            let command = format!("hook {}", takes.command);
            return Err(ARGUMENT_UNKNOWN.render((&command, &word)).into());
        }
        let value =
            attached.or_else(|| words.next().map(|next| next.to_string_lossy().into_owned()));
        let value = value
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ARGUMENT_MISSING.render((ACCEPTS_TEXT, name)))?;
        set(&mut options, name, value)?;
    }
    Ok(options)
}

fn takes_value(name: &str, takes: Takes) -> bool {
    match name {
        SCOPE => takes.scope,
        REPO => true,
        SERVER_NAME => takes.server_name,
        _ => false,
    }
}

fn set(options: &mut Options, name: &str, value: String) -> Result<(), String> {
    match name {
        SCOPE => options.scope = scope(&value)?,
        REPO => options.repo = Some(value),
        _ => options.server_name = Some(server_name(value)?),
    }
    Ok(())
}

// Claude Code reads the ledger-mark matcher as a regular expression, so a name holds none of its
// operators.
fn server_name(value: String) -> Result<String, String> {
    let plain = (value.bytes()).all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte));
    if plain {
        Ok(value)
    } else {
        Err(ARGUMENT_INVALID.render((ACCEPTS_SERVER_NAME, SERVER_NAME, &value)))
    }
}

fn scope(value: &str) -> Result<Scope, String> {
    let found = SCOPES.iter().find(|(name, _)| *name == value);
    found.map(|(_, scope)| *scope).ok_or_else(|| {
        let listed = LISTED_OR.render(("local", "user, project"));
        ARGUMENT_INVALID.render((&ACCEPTS_CHOICE.render(&listed), SCOPE, value))
    })
}
