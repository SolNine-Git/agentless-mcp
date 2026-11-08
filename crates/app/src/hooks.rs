//! The hook entry table that `hook install` writes and that `hook status` and the server compare a
//! settings file with, and the comparison itself: a parse of the settings `hooks` value into
//! handlers, then a pure check of those handlers against the table. Neither reads a file.

mod parse;

use std::path::{Path, PathBuf};
use std::time::Duration;

use text::{posix_name, split_shell_words};

use crate::EnvConfig;

pub use parse::{Expected, Handler, HooksShape, Misshaped, handlers, settings_handlers};

/// The file name of the binary whose hook commands are agentless entries.
pub const BINARY_NAME: &str = "agentless-mcp";
/// The MCP server name that `ledger-mark` matches when the install names none.
pub const DEFAULT_SERVER_NAME: &str = "agentless";
/// A settings `timeout` is this many times its hook's deadline.
pub const TIMEOUT_FACTOR: u32 = 3;
/// The time that `read-card` may spend on a card.
pub const READ_CARD_DEADLINE: Duration = Duration::from_secs(3);
/// The variable that names the user's Claude Code configuration folder.
pub const CONFIG_DIR_VARIABLE: &str = "CLAUDE_CONFIG_DIR";
/// The file name of a user or project settings file.
pub const SETTINGS_FILE: &str = "settings.json";

// The configuration folder under the home folder, when the variable names none.
const HOME_CONFIG_DIR: &str = ".claude";

// The word between the binary and the hook name in every agentless command.
const HOOK_WORD: &str = "hook";
// The settings `type` of a handler that runs a command.
const COMMAND_TYPE: &str = "command";
// A matcher that every tool or source matches, as Claude Code reads one.
const MATCH_ALL: [&str; 2] = ["", "*"];

/// A hook that the installed binary serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hook {
    /// The file card on a first read of a source file.
    ReadCard,
    /// The ledger entry for each file that a trace card covers.
    LedgerMark,
    /// The record of a subagent's brief, which `subagent-map` pairs.
    BriefRecord,
    /// The map sentence for a subagent's first prompt.
    SubagentMap,
    /// The map sentence for a first prompt.
    PromptMap,
    /// The erosion advisory on an edit.
    Erosion,
    /// The removal of the main context's ledger at compaction.
    LedgerReset,
}

/// A hook that an earlier release installed and this one no longer serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retired {
    /// The search gate, which denied Grep, Glob and shell search.
    GateCheck,
    /// The marker of a localizing call, which the search gate read.
    GateMark,
}

/// A Claude Code hook event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    PreToolUse,
    PostToolUse,
    SubagentStart,
    UserPromptSubmit,
    SessionStart,
}

const EVENT_KEYS: [&str; 5] = [
    "PreToolUse",
    "PostToolUse",
    "SubagentStart",
    "UserPromptSubmit",
    "SessionStart",
];

impl Event {
    /// Returns the settings key, such as `PostToolUse`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        EVENT_KEYS[self as usize]
    }
}

impl Hook {
    /// Every hook, in the order of the entry table.
    pub const ALL: [Hook; 7] = [
        Hook::ReadCard,
        Hook::LedgerMark,
        Hook::BriefRecord,
        Hook::SubagentMap,
        Hook::PromptMap,
        Hook::Erosion,
        Hook::LedgerReset,
    ];

    /// Returns the subcommand name, such as `read-card`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Hook::ReadCard => "read-card",
            Hook::LedgerMark => "ledger-mark",
            Hook::BriefRecord => "brief-record",
            Hook::SubagentMap => "subagent-map",
            Hook::PromptMap => "prompt-map",
            Hook::Erosion => "erosion",
            Hook::LedgerReset => "ledger-reset",
        }
    }

    /// Returns the time the hook may spend before it stays silent.
    #[must_use]
    pub fn deadline(self) -> Duration {
        match self {
            Hook::ReadCard => READ_CARD_DEADLINE,
            Hook::LedgerMark => Duration::from_secs(2),
            Hook::BriefRecord | Hook::LedgerReset => Duration::from_secs(1),
            Hook::SubagentMap | Hook::PromptMap | Hook::Erosion => Duration::from_secs(5),
        }
    }

    fn event(self) -> Event {
        match self {
            Hook::ReadCard | Hook::LedgerMark => Event::PostToolUse,
            Hook::BriefRecord | Hook::Erosion => Event::PreToolUse,
            Hook::SubagentMap => Event::SubagentStart,
            Hook::PromptMap => Event::UserPromptSubmit,
            Hook::LedgerReset => Event::SessionStart,
        }
    }

    fn matcher(self, server_name: &str) -> Option<String> {
        let matcher = match self {
            Hook::ReadCard => "Read|Bash",
            Hook::LedgerMark => return Some(format!("mcp__{server_name}__trace")),
            Hook::BriefRecord => "Agent",
            Hook::Erosion => "Write|Edit",
            Hook::LedgerReset => "compact",
            Hook::SubagentMap | Hook::PromptMap => return None,
        };
        Some(matcher.to_owned())
    }
}

impl Retired {
    /// Every retired hook.
    pub const ALL: [Retired; 2] = [Retired::GateCheck, Retired::GateMark];

    /// Returns the subcommand name, such as `gate-check`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Retired::GateCheck => "gate-check",
            Retired::GateMark => "gate-mark",
        }
    }
}

/// One row of the entry table: the hook, its event, its matcher and its settings timeout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The hook.
    pub hook: Hook,
    /// The event it runs on.
    pub event: Event,
    /// The matcher, or `None` for an event that every tool or source matches.
    pub matcher: Option<String>,
    /// The settings timeout, in seconds.
    pub timeout: u32,
}

/// Returns the entry table, in install order, for the MCP server that `server_name` names.
#[must_use]
pub fn entries(server_name: &str) -> Vec<Entry> {
    (Hook::ALL.into_iter())
        .map(|hook| Entry {
            hook,
            event: hook.event(),
            matcher: hook.matcher(server_name),
            timeout: timeout(hook.deadline()),
        })
        .collect()
}

// The deadlines are whole seconds, so the timeout is too.
fn timeout(deadline: Duration) -> u32 {
    let seconds = u32::try_from(deadline.as_secs()).unwrap_or(u32::MAX);
    seconds.saturating_mul(TIMEOUT_FACTOR)
}

/// Returns the command of `hook` for the binary at `binary`, quoted for the shell that runs it.
#[must_use]
pub fn command(binary: &str, hook: Hook) -> String {
    format!("{} {HOOK_WORD} {}", shell_quoted(binary), hook.name())
}

fn shell_quoted(word: &str) -> String {
    let plain = !word.is_empty()
        && (word.bytes()).all(|byte| byte.is_ascii_alphanumeric() || b"_-./+@%,:".contains(&byte));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// How the agentless handlers of a settings file stand against the entry table.
#[derive(Clone, Debug, PartialEq)]
pub struct HookStatus {
    /// One row per table entry, in table order.
    pub rows: Vec<RowStatus>,
    /// The agentless handlers of retired hooks.
    pub retired: Vec<RetiredEntry>,
}

impl HookStatus {
    /// Returns true when every entry is current and no retired handler is left.
    #[must_use]
    pub fn is_current(&self) -> bool {
        self.retired.is_empty() && (self.rows.iter()).all(|row| row.state == EntryState::Current)
    }
}

/// One table entry and how the settings hold it.
#[derive(Clone, Debug, PartialEq)]
pub struct RowStatus {
    /// The table entry.
    pub entry: Entry,
    /// How the settings hold it.
    pub state: EntryState,
}

/// How the settings hold one table entry.
#[derive(Clone, Debug, PartialEq)]
pub enum EntryState {
    /// One handler matches the entry and runs this binary.
    Current,
    /// No handler runs the hook.
    Missing,
    /// The handlers that run the hook differ from the entry, or more than one runs it.
    Stale(Vec<Found>),
    /// One handler matches the entry and runs another binary, at this path.
    OtherPath(String),
}

/// An agentless handler as the settings hold it.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// The event key it sits under.
    pub event: String,
    /// Its group's matcher, with a match-all matcher read as `None`.
    pub matcher: Option<String>,
    /// Its timeout, in seconds, or `None` when it has none.
    pub timeout: Option<f64>,
    /// The binary path of its command.
    pub path: String,
}

/// A handler of a retired hook.
#[derive(Clone, Debug, PartialEq)]
pub struct RetiredEntry {
    /// The retired hook.
    pub hook: Retired,
    /// The handler.
    pub found: Found,
}

/// Returns `$CLAUDE_CONFIG_DIR/settings.json` when the variable is set and not empty, else
/// `$HOME/.claude/settings.json`, or `None` when neither is set and not empty.
#[must_use]
pub fn user_settings(env: &EnvConfig) -> Option<PathBuf> {
    let named = |name: &str| env.var(name).filter(|value| !value.is_empty());
    let config_dir = (named(CONFIG_DIR_VARIABLE).map(PathBuf::from))
        .or_else(|| named("HOME").map(|home| Path::new(home).join(HOME_CONFIG_DIR)));
    config_dir.map(|dir| dir.join(SETTINGS_FILE))
}

/// The hook that an agentless entry runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Named {
    /// A hook of the entry table.
    Current(Hook),
    /// A hook that an earlier release installed.
    Retired(Retired),
}

/// Returns the hook of `handler` when it is an agentless entry (design rule 1): a command
/// `<path> hook <name>` whose path's file name is [`BINARY_NAME`] and whose name is a current or
/// retired hook. Any other handler gives `None`.
#[must_use]
pub fn named(handler: &Handler) -> Option<Named> {
    agentless(handler).map(|(named, _)| named)
}

/// Compares `handlers` with the entry table of `server_name`. `own` is the canonical path of the
/// binary that compares. An agentless handler is a command `<path> hook <name>` whose path's file
/// name is [`BINARY_NAME`] and whose name is a current or retired hook; no other is read.
#[must_use]
pub fn compare(handlers: &[Handler], own: &str, server_name: &str) -> HookStatus {
    let mut current: Vec<(Hook, Found)> = Vec::new();
    let mut retired: Vec<RetiredEntry> = Vec::new();
    for handler in handlers {
        let Some((named, path)) = agentless(handler) else {
            continue;
        };
        let found = found(handler, path);
        match named {
            Named::Current(hook) if !current.contains(&(hook, found.clone())) => {
                current.push((hook, found));
            }
            Named::Retired(hook) => retired.push(RetiredEntry { hook, found }),
            Named::Current(_) => {}
        }
    }
    let rows = (entries(server_name).into_iter())
        .map(|entry| {
            let found: Vec<Found> = (current.iter())
                .filter(|(hook, _)| *hook == entry.hook)
                .map(|(_, found)| found.clone())
                .collect();
            let state = state(&entry, found, own);
            RowStatus { entry, state }
        })
        .collect();
    HookStatus { rows, retired }
}

// The hook of an agentless entry and the binary path of its command.
fn agentless(handler: &Handler) -> Option<(Named, String)> {
    if handler.kind.as_deref() != Some(COMMAND_TYPE) {
        return None;
    }
    let words = split_shell_words(handler.command.as_deref()?).ok()?;
    let [path, word, name] = <[String; 3]>::try_from(words).ok()?;
    if word != HOOK_WORD || posix_name(&path) != BINARY_NAME {
        return None;
    }
    let named = (Hook::ALL.into_iter().find(|hook| hook.name() == name))
        .map(Named::Current)
        .or_else(|| {
            (Retired::ALL.into_iter().find(|hook| hook.name() == name)).map(Named::Retired)
        })?;
    Some((named, path))
}

fn found(handler: &Handler, path: String) -> Found {
    let matcher = (handler.matcher.as_deref()).filter(|matcher| !MATCH_ALL.contains(matcher));
    Found {
        event: handler.event.clone(),
        matcher: matcher.map(str::to_owned),
        timeout: handler.timeout,
        path,
    }
}

// A handler that two settings files define runs once, so a copy is one handler.
fn state(entry: &Entry, found: Vec<Found>, own: &str) -> EntryState {
    let [only] = found.as_slice() else {
        return if found.is_empty() {
            EntryState::Missing
        } else {
            EntryState::Stale(found)
        };
    };
    let shaped = only.event == entry.event.as_str()
        && only.matcher == entry.matcher
        && only.timeout == Some(f64::from(entry.timeout));
    match (shaped, only.path == own) {
        (false, _) => EntryState::Stale(found),
        (true, true) => EntryState::Current,
        (true, false) => EntryState::OtherPath(only.path.clone()),
    }
}

#[cfg(test)]
mod tests;
