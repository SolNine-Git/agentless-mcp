//! hook install, hook status and hook uninstall: the agentless entries of a Claude Code settings
//! file against the entry table of `app::hooks`. Install and uninstall change no other handler and
//! no other key, and status reads only the hooks key.

mod args;
mod disk;
mod edit;
mod layout;

use std::ffi::OsString;
use std::fs;
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};

use app::EnvConfig;
use app::hooks::{
    self, BINARY_NAME, DEFAULT_SERVER_NAME, EntryState, Handler, HookStatus, Retired,
};
use json::{JsonValue, get};
use prompts::messages::{
    ACCEPTS_TEXT, ARGUMENT_MISSING, HOOK_INSTALL_BAD_JSON, HOOK_INSTALL_COUNTS,
    HOOK_INSTALL_DRY_RUN, HOOK_INSTALL_RACED, HOOK_INSTALL_UNCHANGED, HOOK_INSTALL_WROTE,
    HOOK_STATUS_CURRENT, HOOK_STATUS_MISSING, HOOK_STATUS_OTHER_PATH, HOOK_STATUS_RETIRED,
    HOOK_STATUS_STALE, HOOK_UNINSTALL_REPORT, HOOKS_UNREADABLE,
};

use crate::Output;
use crate::text::{codec_text, shown};
use args::{Options, Scope, Takes};
use disk::{Stopped, Written};
use edit::{Removed, Unshaped};

const INSTALL: Takes = Takes {
    command: "install",
    scope: true,
    server_name: true,
    dry_run: true,
};
const STATUS: Takes = Takes {
    command: "status",
    scope: false,
    server_name: false,
    dry_run: false,
};
const UNINSTALL: Takes = Takes {
    command: "uninstall",
    scope: true,
    server_name: false,
    dry_run: false,
};

const CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";
const HOME_ENV: &str = "HOME";
const USER_DIR: &str = ".claude";
const PROJECT_DIR: &str = ".claude";
const SETTINGS: &str = "settings.json";
const LOCAL_SETTINGS: &str = "settings.local.json";
const HOOKS_KEY: &str = "hooks";
// The places of a shape error, in the notation of a JSON path.
const ROOT_PLACE: &str = "$";
const HOOKS_PLACE: &str = "$.hooks";

const EXIT_FAILED: u8 = 1;
const EXIT_USAGE: u8 = 2;

/// Writes the entry table's agentless entries into the settings file that `args` names, for the
/// running binary, and returns the report: exit 0 when written or current, 1 or 2 when refused.
#[must_use]
pub fn install(args: &[OsString], env: &EnvConfig) -> Output {
    with_binary(|binary| installed(args, env, binary))
}

/// Reports each agentless entry of the user, project and local settings against the entry table:
/// exit 0 when every entry is current, else 1.
#[must_use]
pub fn status(args: &[OsString], env: &EnvConfig) -> Output {
    with_binary(|binary| statused(args, env, binary))
}

/// Removes every agentless entry from the settings file that `args` names, and returns the report.
#[must_use]
pub fn uninstall(args: &[OsString], env: &EnvConfig) -> Output {
    uninstalled(args, env)
}

// The canonical path of the running binary, which a uv tool install keeps across upgrades.
fn with_binary(run: impl FnOnce(&str) -> Output) -> Output {
    let binary = std::env::current_exe().and_then(|path| io::resolve(&path));
    match binary {
        Ok(path) => match path.to_str() {
            Some(binary) => run(binary),
            None => failed(&io::os_error_text(
                &Error::from(ErrorKind::InvalidData),
                &path,
            )),
        },
        Err(error) => failed(&io::os_error_str(&error)),
    }
}

// Why a file's text takes no edit.
enum Refusal {
    BadJson(String),
    Unshaped(Unshaped),
    Missing,
}

pub(crate) fn installed(args: &[OsString], env: &EnvConfig, binary: &str) -> Output {
    let options = match args::parsed(args, INSTALL) {
        Ok(options) => options,
        Err(message) => return usage(&message),
    };
    let target = match target(env, &options) {
        Ok(target) => target,
        Err(output) => return output,
    };
    let server = (options.server_name.as_deref()).unwrap_or(DEFAULT_SERVER_NAME);
    let edit = |before: Option<&[u8]>| {
        let document = document(before)?;
        let edited = edit::installed(&document, binary, server).map_err(Refusal::Unshaped)?;
        Ok((layout::indented(&edited.0), counts(&edited.1, server)))
    };
    let shown = shown(&target);
    if options.dry_run {
        return dry_run(&target, &edit);
    }
    match disk::written(&target, &edit, &disk::read) {
        Ok((Written::Wrote, counts)) => done(&HOOK_INSTALL_WROTE.render((&shown, &counts))),
        Ok((Written::Unchanged, _)) => done(&HOOK_INSTALL_UNCHANGED.render(&shown)),
        Ok((Written::Raced, _)) => failed(&HOOK_INSTALL_RACED.render(&shown)),
        Err(stopped) => stopped_output(stopped, &shown, true),
    }
}

// The new text on stdout, so that it can be read or saved; the report on stderr.
fn dry_run(target: &Path, edit: &disk::Edit<'_, String, Refusal>) -> Output {
    let shown = shown(target);
    let before = match disk::read(target) {
        Ok(before) => before,
        Err(error) => return failed(&io::os_error_text(&error, target)),
    };
    match edit(before.as_deref()) {
        Ok((text, counts)) => Output {
            code: 0,
            stdout: text,
            stderr: format!("{}\n", HOOK_INSTALL_DRY_RUN.render((&shown, &counts))),
        },
        Err(refusal) => failed(&refused(&refusal, &shown, true)),
    }
}

pub(crate) fn uninstalled(args: &[OsString], env: &EnvConfig) -> Output {
    let options = match args::parsed(args, UNINSTALL) {
        Ok(options) => options,
        Err(message) => return usage(&message),
    };
    let target = match target(env, &options) {
        Ok(target) => target,
        Err(output) => return output,
    };
    // A missing file holds no entry, and uninstall makes none.
    if let Err(error) = fs::metadata(&target) {
        return failed(&io::os_error_text(&error, &target));
    }
    let edit = |before: Option<&[u8]>| {
        let document = document(Some(before.ok_or(Refusal::Missing)?))?;
        let edited = edit::uninstalled(&document).map_err(Refusal::Unshaped)?;
        Ok((layout::indented(&edited.0), edited.1))
    };
    let shown = shown(&target);
    match disk::written(&target, &edit, &disk::read) {
        Ok((Written::Wrote, removed)) => {
            let count = removed.current.len() + removed.retired.len();
            done(&HOOK_UNINSTALL_REPORT.render((&shown, count, removed.kept)))
        }
        Ok((Written::Unchanged, _)) => Output::default(),
        Ok((Written::Raced, _)) => failed(&HOOK_INSTALL_RACED.render(&shown)),
        Err(stopped) => stopped_output(stopped, &shown, false),
    }
}

pub(crate) fn statused(args: &[OsString], env: &EnvConfig, binary: &str) -> Output {
    let options = match args::parsed(args, STATUS) {
        Ok(options) => options,
        Err(message) => return usage(&message),
    };
    let files = match status_files(env, &options) {
        Ok(files) => files,
        Err(output) => return output,
    };
    let mut handlers = Vec::new();
    let mut lines = Vec::new();
    for file in &files {
        match file_handlers(file) {
            Ok(found) => handlers.extend(found),
            Err(error) => lines.push(HOOKS_UNREADABLE.render((&shown(file), &error))),
        }
    }
    let readable = lines.is_empty();
    let status = hooks::compare(&handlers, binary, DEFAULT_SERVER_NAME);
    lines.extend(status_lines(&status));
    Output {
        code: if readable && status.is_current() {
            0
        } else {
            EXIT_FAILED
        },
        stdout: lines.join("\n") + "\n",
        stderr: String::new(),
    }
}

fn status_lines(status: &HookStatus) -> Vec<String> {
    let rows = status.rows.iter().map(|row| {
        let name = row.entry.hook.name();
        match &row.state {
            EntryState::Current => HOOK_STATUS_CURRENT.render(name),
            EntryState::Missing => HOOK_STATUS_MISSING.render(name),
            EntryState::Stale(_) => HOOK_STATUS_STALE.render(name),
            EntryState::OtherPath(path) => HOOK_STATUS_OTHER_PATH.render((path, name)),
        }
    });
    let retired = (Retired::ALL.into_iter())
        .filter(|hook| status.retired.iter().any(|entry| entry.hook == *hook))
        .map(|hook| HOOK_STATUS_RETIRED.render(hook.name()));
    rows.chain(retired).collect()
}

// The handlers of one file: none when no file is there, or the text of why they cannot be read.
fn file_handlers(path: &Path) -> Result<Vec<Handler>, String> {
    let read = disk::read(path).map_err(|error| io::os_error_text(&error, path))?;
    let Some(bytes) = read else {
        return Ok(Vec::new());
    };
    let document = document(Some(&bytes)).map_err(|refusal| detail(&refusal))?;
    let JsonValue::Object(top) = &document else {
        return Err(detail(&Refusal::Unshaped(Unshaped::TopLevel)));
    };
    match get(top, HOOKS_KEY) {
        Some(value) => hooks::handlers(value)
            .map_err(|shape| detail(&Refusal::Unshaped(Unshaped::Hooks(shape)))),
        None => Ok(Vec::new()),
    }
}

// A missing file is an empty document.
fn document(before: Option<&[u8]>) -> Result<JsonValue, Refusal> {
    let Some(bytes) = before else {
        return Ok(JsonValue::Object(Vec::new()));
    };
    let text = io::decode_strict(bytes.to_vec())
        .map_err(|error| Refusal::BadJson(codec_text(bytes, error)))?;
    json::parse(&text).map_err(|error| Refusal::BadJson(error.to_string()))
}

fn counts(removed: &Removed, server: &str) -> String {
    let rows = hooks::entries(server);
    let replaced = (rows.iter())
        .filter(|row| removed.current.contains(&row.hook))
        .count();
    let names: Vec<&str> = (Retired::ALL.into_iter())
        .filter(|hook| removed.retired.contains(hook))
        .map(Retired::name)
        .collect();
    let listed = if names.is_empty() {
        String::new()
    } else {
        format!(" ({})", names.join(", "))
    };
    let (added, retired) = (rows.len() - replaced, removed.retired.len());
    HOOK_INSTALL_COUNTS.render((&listed, added, replaced, retired, removed.kept))
}

// The file that the options name, and its target when it is a symlink.
fn target(env: &EnvConfig, options: &Options) -> Result<PathBuf, Output> {
    let path = settings_path(env, options.scope, options.repo.as_deref())?;
    io::resolve(&path).map_err(|error| failed(&io::os_error_text(&error, &path)))
}

// The user file, then the project and local files of the repository.
fn status_files(env: &EnvConfig, options: &Options) -> Result<Vec<PathBuf>, Output> {
    let repo = options.repo.as_deref();
    [Scope::User, Scope::Project, Scope::Local]
        .into_iter()
        .map(|scope| settings_path(env, scope, repo))
        .collect()
}

fn settings_path(env: &EnvConfig, scope: Scope, repo: Option<&str>) -> Result<PathBuf, Output> {
    match scope {
        Scope::User => user_dir(env).map(|dir| dir.join(SETTINGS)),
        Scope::Project => Ok(repo_dir(env, repo)?.join(PROJECT_DIR).join(SETTINGS)),
        Scope::Local => Ok(repo_dir(env, repo)?.join(PROJECT_DIR).join(LOCAL_SETTINGS)),
    }
}

// CLAUDE_CONFIG_DIR when it is set and not empty, else ~/.claude.
fn user_dir(env: &EnvConfig) -> Result<PathBuf, Output> {
    let set = |name: &str| env.var(name).filter(|value| !value.is_empty());
    if let Some(dir) = set(CONFIG_DIR_ENV) {
        return Ok(PathBuf::from(dir));
    }
    let home = set(HOME_ENV).map(|home| Path::new(home).join(USER_DIR));
    home.ok_or_else(|| failed(&ARGUMENT_MISSING.render((ACCEPTS_TEXT, CONFIG_DIR_ENV))))
}

// The repository folder: --repo below the start-up directory, else the start-up directory.
fn repo_dir(env: &EnvConfig, repo: Option<&str>) -> Result<PathBuf, Output> {
    let cwd = env
        .cwd()
        .map_err(|unread| failed(&io::os_error_str(&unread.error())))?;
    let dir = repo.map_or_else(|| cwd.to_path_buf(), |repo| cwd.join(repo));
    match fs::metadata(&dir) {
        Ok(metadata) if metadata.is_dir() => Ok(dir),
        Ok(_) => Err(failed(&io::os_error_text(
            &Error::from(ErrorKind::NotADirectory),
            &dir,
        ))),
        Err(error) => Err(failed(&io::os_error_text(&error, &dir))),
    }
}

fn stopped_output(stopped: Stopped<Refusal>, shown: &str, installing: bool) -> Output {
    match stopped {
        Stopped::Refused(refusal) => failed(&refused(&refusal, shown, installing)),
        Stopped::Failed(error, at) => failed(&io::os_error_text(&error, &at)),
    }
}

fn refused(refusal: &Refusal, shown: &str, installing: bool) -> String {
    match refusal {
        Refusal::BadJson(error) if installing => HOOK_INSTALL_BAD_JSON.render((shown, error)),
        other => HOOKS_UNREADABLE.render((shown, &detail(other))),
    }
}

// The parse error, or the place whose value has another shape than Claude Code reads.
fn detail(refusal: &Refusal) -> String {
    match refusal {
        Refusal::BadJson(error) => error.clone(),
        Refusal::Unshaped(Unshaped::TopLevel) => ROOT_PLACE.to_owned(),
        Refusal::Unshaped(Unshaped::Hooks(shape)) if shape.at.is_empty() => HOOKS_PLACE.to_owned(),
        Refusal::Unshaped(Unshaped::Hooks(shape)) => format!("{HOOKS_PLACE}.{}", shape.at),
        Refusal::Missing => io::strerror(&Error::from(ErrorKind::NotFound)),
    }
}

fn done(line: &str) -> Output {
    Output {
        code: 0,
        stdout: format!("{line}\n"),
        stderr: String::new(),
    }
}

fn failed(message: &str) -> Output {
    refusal_output(message, EXIT_FAILED)
}

fn usage(message: &str) -> Output {
    refusal_output(message, EXIT_USAGE)
}

fn refusal_output(message: &str, code: u8) -> Output {
    Output {
        code,
        stdout: String::new(),
        stderr: format!("{BINARY_NAME}: {message}\n"),
    }
}

#[cfg(test)]
mod tests;
