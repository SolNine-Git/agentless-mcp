//! The settings file on disk: the read, the first backup, and the atomic write, which retries once
//! when the file changed between the read and the rename.

use std::fs;
use std::io::{Error, ErrorKind, Result as IoResult};
use std::path::{Path, PathBuf};

const BACKUP_SUFFIX: &str = ".agentless-backup";
const TRIES: usize = 2;

// The bytes of the file, or None when no file is there.
pub(crate) type Reread<'a> = dyn Fn(&Path) -> IoResult<Option<Vec<u8>>> + 'a;

// The new text of the file from its bytes, with a report, or the edit's refusal.
pub(crate) type Edit<'a, T, R> = dyn Fn(Option<&[u8]>) -> Result<(String, T), R> + 'a;

// What one write did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Written {
    Unchanged,
    Wrote,
    Raced,
}

// Why a write stopped: the edit's own refusal, or an error of the file system at a path.
pub(crate) enum Stopped<R> {
    Refused(R),
    Failed(Error, PathBuf),
}

pub(crate) fn read(path: &Path) -> IoResult<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

// The edit turns the file's bytes into the new text and its report. The file is read again just
// before the rename, and a file that changed since the first read takes the edit once more.
pub(crate) fn written<T, R>(
    target: &Path,
    edit: &Edit<'_, T, R>,
    reread: &Reread<'_>,
) -> Result<(Written, T), Stopped<R>> {
    let failed = |error: Error| Stopped::Failed(error, target.to_path_buf());
    let mut tries = 0;
    loop {
        let before = reread(target).map_err(failed)?;
        let (text, report) = edit(before.as_deref()).map_err(Stopped::Refused)?;
        if before.as_deref() == Some(text.as_bytes()) {
            return Ok((Written::Unchanged, report));
        }
        if reread(target).map_err(failed)? != before {
            tries += 1;
            if tries == TRIES {
                return Ok((Written::Raced, report));
            }
            continue;
        }
        backed_up(target, before.as_deref())?;
        io::replace_file(target, text.as_bytes()).map_err(failed)?;
        return Ok((Written::Wrote, report));
    }
}

// The first write keeps the file as it was beside it; a later write keeps that first backup.
fn backed_up<R>(target: &Path, before: Option<&[u8]>) -> Result<(), Stopped<R>> {
    let Some(before) = before else {
        return Ok(());
    };
    let mut name = target.as_os_str().to_owned();
    name.push(BACKUP_SUFFIX);
    let backup = PathBuf::from(name);
    if fs::symlink_metadata(&backup).is_ok() {
        return Ok(());
    }
    io::replace_file(&backup, before).map_err(|error| Stopped::Failed(error, backup))
}
