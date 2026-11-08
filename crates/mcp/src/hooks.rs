//! The hook note: how the agentless entries of the Claude Code settings stand against the entry
//! table of this binary. The server reads only the `hooks` key of the user settings file and of
//! each root's project and local settings files, at start-up and again when a file changes, and the
//! first answer of each client session carries the note when the entries are not current.

use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use app::EnvConfig;
use app::hooks::{
    EntryState, Handler, HookStatus, SETTINGS_FILE, ServerName, compare, settings_handlers,
    user_settings,
};
use prompts::messages::{
    HOOKS_MISSING, HOOKS_NOTE, HOOKS_OTHER_PATH, HOOKS_RETIRED, HOOKS_RETIRED_MORE, HOOKS_STALE,
    HOOKS_STALE_MORE, HOOKS_UNREADABLE, LISTED_AND,
};

use crate::refusal::listed;
use crate::roots::stat_key;

const CONFIG_DIR: &str = ".claude";
const LOCAL_SETTINGS: &str = "settings.local.json";

/// What the server compares the settings with: the user settings file and its own binary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookCheck {
    /// The user settings file, or `None` when the environment names no home.
    pub user_settings: Option<PathBuf>,
    /// The canonical path of the running binary.
    pub binary: PathBuf,
}

impl HookCheck {
    /// Returns the check of `binary` against `$CLAUDE_CONFIG_DIR/settings.json`, else
    /// `$HOME/.claude/settings.json`.
    #[must_use]
    pub fn of(env: &EnvConfig, binary: PathBuf) -> HookCheck {
        HookCheck {
            user_settings: user_settings(env),
            binary,
        }
    }
}

// The claim of one client session on the hook note: the first answer with a receipt takes it.
#[derive(Clone, Debug, Default)]
pub(crate) struct SessionNote(Arc<AtomicBool>);

impl SessionNote {
    pub(crate) fn claim(&self) -> bool {
        !self.0.swap(true, Ordering::AcqRel)
    }
}

// One settings file as last read: the (mtime_ns, size) it was read at, or `None` when it was
// absent, and its handlers or why they are unknown.
struct Read {
    key: Option<(i128, u64)>,
    handlers: Result<Vec<Handler>, String>,
}

// The settings files as last read, by path.
pub(crate) struct Hooks {
    check: Option<HookCheck>,
    read: Mutex<HashMap<PathBuf, Read>>,
}

impl Hooks {
    // With no check, the server gives no note. The files of `roots` are read now.
    pub(crate) fn new(check: Option<HookCheck>, roots: &[PathBuf]) -> Hooks {
        let hooks = Hooks {
            check,
            read: Mutex::default(),
        };
        if let Some(check) = &hooks.check {
            for root in roots {
                drop(hooks.handlers(check, root));
            }
        }
        hooks
    }

    // The note of an answer from `root`, or `None` when every entry is current.
    pub(crate) fn note(&self, root: &Path) -> Option<String> {
        let check = self.check.as_ref()?;
        match self.handlers(check, root) {
            Ok(found) => {
                let own = check.binary.to_string_lossy();
                let status = compare(&found, &own, ServerName::Any);
                status_note(&resolved(status, &check.binary), &own)
            }
            Err((path, error)) => Some(HOOKS_UNREADABLE.render((&path, &error))),
        }
    }

    // The handlers of the user file, then the project and local files of `root`. The first file
    // that cannot be read makes the state unknown.
    fn handlers(&self, check: &HookCheck, root: &Path) -> Result<Vec<Handler>, (String, String)> {
        let project = root.join(CONFIG_DIR);
        let files = (check.user_settings.iter().cloned())
            .chain([project.join(SETTINGS_FILE), project.join(LOCAL_SETTINGS)]);
        let mut read = self.read.lock().unwrap_or_else(PoisonError::into_inner);
        let mut found = Vec::new();
        for path in files {
            let handlers =
                current(&mut read, &path).map_err(|error| (path.display().to_string(), error))?;
            found.extend(handlers);
        }
        Ok(found)
    }
}

// The handlers of `path` as of now: the last read while (mtime_ns, size) holds, else a new read.
// An absent file holds none.
fn current(read: &mut HashMap<PathBuf, Read>, path: &Path) -> Result<Vec<Handler>, String> {
    let key = match stat_key(path) {
        Ok(key) => Some(key),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(io::os_error_str(&error)),
    };
    if let Some(kept) = read.get(path).filter(|kept| kept.key == key) {
        return kept.handlers.clone();
    }
    let handlers = key.map_or_else(|| Ok(Vec::new()), |_| parsed(path));
    let kept = Read {
        key,
        handlers: handlers.clone(),
    };
    read.insert(path.to_owned(), kept);
    handlers
}

// The `hooks` value of one settings file; every other key is left unread.
fn parsed(path: &Path) -> Result<Vec<Handler>, String> {
    let bytes = fs::read(path).map_err(|error| io::os_error_str(&error))?;
    let text = io::decode_text(bytes.clone()).map_err(|error| io::codec_text(&bytes, error))?;
    let document = json::parse(&text).map_err(|error| error.to_string())?;
    settings_handlers(&document).map_err(|shape| shape.text())
}

// An entry whose path resolves to this binary is current: a link to it runs the same file.
fn resolved(mut status: HookStatus, binary: &Path) -> HookStatus {
    for row in &mut status.rows {
        if let EntryState::OtherPath(path) = &row.state
            && io::resolve(Path::new(path)).is_ok_and(|path| path == binary)
        {
            row.state = EntryState::Current;
        }
    }
    status
}

// The note for `status`, or `None` when every entry is current. Missing, stale and retired
// entries ask for an install; entries that run another binary name it.
fn status_note(status: &HookStatus, own: &str) -> Option<String> {
    let parts = summary(status);
    if !parts.is_empty() {
        let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
        return Some(HOOKS_NOTE.render(&listed(&parts, &LISTED_AND)));
    }
    (status.rows.iter()).find_map(|row| match &row.state {
        EntryState::OtherPath(path) => Some(HOOKS_OTHER_PATH.render((own, path))),
        _ => None,
    })
}

// The missing, the stale and the retired entries, each named once; the first part says how many
// entries the table has.
fn summary(status: &HookStatus) -> Vec<String> {
    let named = |wanted: fn(&EntryState) -> bool| -> Vec<&str> {
        (status.rows.iter())
            .filter(|row| wanted(&row.state))
            .map(|row| row.entry.hook.name())
            .collect()
    };
    let missing = named(|state| matches!(state, EntryState::Missing));
    let stale = named(|state| matches!(state, EntryState::Stale(_)));
    let mut retired: Vec<&str> = Vec::new();
    for entry in &status.retired {
        if !retired.contains(&entry.hook.name()) {
            retired.push(entry.hook.name());
        }
    }
    let total = status.rows.len();
    let mut parts = Vec::new();
    if !missing.is_empty() {
        parts.push(HOOKS_MISSING.render((missing.len(), total, &missing.join(", "))));
    }
    if !stale.is_empty() {
        let (count, listing) = (stale.len(), stale.join(", "));
        let part = if parts.is_empty() {
            HOOKS_STALE.render((count, total, &listing))
        } else {
            HOOKS_STALE_MORE.render((count, &listing))
        };
        parts.push(part);
    }
    if !retired.is_empty() {
        let (count, listing) = (retired.len(), retired.join(", "));
        let part = if parts.is_empty() {
            HOOKS_RETIRED.render((count, &listing))
        } else {
            HOOKS_RETIRED_MORE.render((count, &listing))
        };
        parts.push(part);
    }
    parts
}

#[cfg(test)]
mod tests;
