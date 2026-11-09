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
    self, BINARY_NAME, CONFIG_DIR_VARIABLE, DEFAULT_SERVER_NAME, EntryState, Handler, HookStatus,
    Misshaped, Retired, ServerName,
};
use json::JsonValue;
use prompts::messages::{
    ACCEPTS_TEXT, ARGUMENT_MISSING, HOOK_INSTALL_BAD_JSON, HOOK_INSTALL_COUNTS,
    HOOK_INSTALL_DRY_RUN, HOOK_INSTALL_NOT_FINITE, HOOK_INSTALL_RACED, HOOK_INSTALL_UNCHANGED,
    HOOK_INSTALL_WROTE, HOOK_STATUS_CURRENT, HOOK_STATUS_MISSING, HOOK_STATUS_OTHER_PATH,
    HOOK_STATUS_RETIRED, HOOK_STATUS_STALE, HOOK_UNINSTALL_NONE, HOOK_UNINSTALL_REPORT,
    HOOKS_UNREADABLE,
};

use crate::Output;
use crate::text::shown;
use args::{Options, Scope, Takes};
use disk::{Stopped, Written};
use edit::Removed;

const INSTALL: Takes = Takes {
    command: "install",
    scope: true,
    server_name: true,
    dry_run: true,
};
const STATUS: Takes = Takes {
    command: "status",
    scope: false,
    server_name: true,
    dry_run: false,
};
const UNINSTALL: Takes = Takes {
    command: "uninstall",
    scope: true,
    server_name: false,
    dry_run: false,
};

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

// Why a file's text takes no edit. Uninstall of a file with no agentless entry, or of no file,
// writes nothing and succeeds.
pub(crate) enum NoEdit {
    BadJson(String),
    Unshaped(Misshaped),
    NotFinite,
    NothingToRemove,
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
    // A missing file is an empty document.
    let edit = |before: Option<&[u8]>| {
        let document = before.map_or_else(|| Ok(JsonValue::Object(Vec::new())), document)?;
        let edited = edit::installed(&document, binary, server).map_err(NoEdit::Unshaped)?;
        Ok((laid_out(&edited.0)?, counts(&edited.1, server)))
    };
    let shown = shown(&target);
    if options.dry_run {
        return dry_run(&target, &edit);
    }
    match disk::written(&target, &edit, &disk::read) {
        Ok((Written::Wrote, counts)) => done(&HOOK_INSTALL_WROTE.render((&shown, &counts))),
        Ok((Written::Unchanged, _)) => done(&HOOK_INSTALL_UNCHANGED.render(&shown)),
        Ok((Written::Raced, _)) => failed(&HOOK_INSTALL_RACED.render((&shown, INSTALL.command))),
        Err(stopped) => stopped_output(stopped, &shown, INSTALL.command),
    }
}

// The new text on stdout, so that it can be read or saved; the report on stderr.
fn dry_run(target: &Path, edit: &disk::Edit<'_, String, NoEdit>) -> Output {
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
        Err(no_edit) => no_edit_output(&no_edit, &shown, INSTALL.command),
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
    let written = disk::written(&target, &uninstall_edit, &disk::read);
    uninstall_output(written, &shown(&target))
}

pub(crate) fn uninstall_output(
    written: Result<(Written, Removed), Stopped<NoEdit>>,
    shown: &str,
) -> Output {
    match written {
        Ok((Written::Wrote, removed)) => {
            let count = removed.current.len() + removed.retired.len();
            done(&HOOK_UNINSTALL_REPORT.render((shown, count, removed.kept)))
        }
        Ok((Written::Unchanged, _)) => done(&HOOK_UNINSTALL_NONE.render(shown)),
        Ok((Written::Raced, _)) => failed(&HOOK_INSTALL_RACED.render((shown, UNINSTALL.command))),
        Err(stopped) => stopped_output(stopped, shown, UNINSTALL.command),
    }
}

// A missing file holds no entry, and uninstall makes none.
fn uninstall_edit(before: Option<&[u8]>) -> Result<(String, Removed), NoEdit> {
    let document = document(before.ok_or(NoEdit::NothingToRemove)?)?;
    let (edited, removed) = edit::uninstalled(&document).map_err(NoEdit::Unshaped)?;
    if removed.current.is_empty() && removed.retired.is_empty() {
        return Err(NoEdit::NothingToRemove);
    }
    Ok((laid_out(&edited)?, removed))
}

fn laid_out(document: &JsonValue) -> Result<String, NoEdit> {
    layout::indented(document).map_err(|_| NoEdit::NotFinite)
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
    let server = (options.server_name.as_deref()).unwrap_or(DEFAULT_SERVER_NAME);
    let status = hooks::compare(&handlers, binary, ServerName::Exact(server));
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
    read.map_or_else(|| Ok(Vec::new()), |bytes| hooks::file_handlers(&bytes))
}

fn document(bytes: &[u8]) -> Result<JsonValue, NoEdit> {
    hooks::json_document(bytes).map_err(|error| NoEdit::BadJson(error.to_string()))
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

fn status_files(env: &EnvConfig, options: &Options) -> Result<Vec<PathBuf>, Output> {
    let user = user_settings(env)?;
    let root = repo_dir(env, options.repo.as_deref())?;
    Ok(hooks::settings_files(Some(user), &root))
}

fn settings_path(env: &EnvConfig, scope: Scope, repo: Option<&str>) -> Result<PathBuf, Output> {
    match scope {
        Scope::User => user_settings(env),
        Scope::Project => Ok(hooks::project_settings(&repo_dir(env, repo)?)),
        Scope::Local => Ok(hooks::local_settings(&repo_dir(env, repo)?)),
    }
}

fn user_settings(env: &EnvConfig) -> Result<PathBuf, Output> {
    hooks::user_settings(env)
        .ok_or_else(|| failed(&ARGUMENT_MISSING.render((ACCEPTS_TEXT, CONFIG_DIR_VARIABLE))))
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

fn stopped_output(stopped: Stopped<NoEdit>, shown: &str, command: &str) -> Output {
    match stopped {
        Stopped::Refused(no_edit) => no_edit_output(&no_edit, shown, command),
        Stopped::Failed(error, at) => failed(&io::os_error_text(&error, &at)),
    }
}

fn no_edit_output(no_edit: &NoEdit, shown: &str, command: &str) -> Output {
    match no_edit {
        NoEdit::BadJson(error) => failed(&HOOK_INSTALL_BAD_JSON.render((shown, command, error))),
        NoEdit::Unshaped(shape) => failed(&HOOKS_UNREADABLE.render((shown, &shape.text()))),
        NoEdit::NotFinite => failed(&HOOK_INSTALL_NOT_FINITE.render((shown, command))),
        NoEdit::NothingToRemove => done(&HOOK_UNINSTALL_NONE.render(shown)),
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
