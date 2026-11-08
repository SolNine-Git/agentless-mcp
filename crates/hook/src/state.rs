//! The files that the hooks keep between runs: a directory below the temporary directory that
//! only this user can write, and one empty file per claim, made by exclusive create.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use app::EnvConfig;

const TEMPORARY_DIR: &str = "/tmp";

// The outcome of one claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Claim {
    // This run made the file.
    New,
    // An earlier run made it.
    Held,
    // The file can be neither made nor trusted.
    Unavailable,
}

// `name` below TMPDIR when it is set and not empty, else below /tmp.
pub(crate) fn temporary_dir(env: &EnvConfig, name: &str) -> PathBuf {
    let base = env.var("TMPDIR").filter(|value| !value.is_empty());
    base.map_or_else(|| PathBuf::from(TEMPORARY_DIR), PathBuf::from)
        .join(name)
}

// Another user can plant entries in a shared directory, so only a private one counts.
pub(crate) fn private_dir(dir: &Path) -> bool {
    io::create_missing_owner_dir(dir).is_ok() && matches!(io::is_private_directory(dir), Ok(true))
}

// The first claim wins, so parallel runs stay safe. A symlink at the name is no claim.
pub(crate) fn claim(path: &Path) -> Claim {
    match io::write_new_file(path, b"") {
        Ok(()) => Claim::New,
        Err(error) if error.kind() == ErrorKind::AlreadyExists && is_plain_file(path) => {
            Claim::Held
        }
        Err(_) => Claim::Unavailable,
    }
}

fn is_plain_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}
