//! Random bytes from the OS, and the exclusive creation of a file with a random name.

use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const SOURCE: &str = "/dev/urandom";
// The characters and the length of the random part of a temporary file name.
const NAME_CHARACTERS: &[u8; 37] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
const NAME_LENGTH: usize = 8;
const FILE_MODE: u32 = 0o600;
// A taken name is skipped, and a run of taken names is a refusal.
const MAX_NAME_ATTEMPTS: usize = 100;

/// Fills `buffer` with bytes from the OS random source.
///
/// # Errors
///
/// Returns the error of the open or of the read of the source.
pub fn random_bytes(buffer: &mut [u8]) -> io::Result<()> {
    File::open(SOURCE)?.read_exact(buffer)
}

// Creates `<prefix><8 random characters><suffix>` in `directory` with mode 0600, exclusively, so
// an existing file or symlink at the name is never opened.
pub(crate) fn create_named(
    directory: &Path,
    prefix: &OsStr,
    suffix: &str,
) -> io::Result<(PathBuf, File)> {
    for _ in 0..MAX_NAME_ATTEMPTS {
        let path = directory.join(name(prefix, suffix)?);
        let created = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(FILE_MODE)
            .open(&path);
        match created {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    let message = "No usable temporary file name found";
    Err(io::Error::new(io::ErrorKind::AlreadyExists, message))
}

fn name(prefix: &OsStr, suffix: &str) -> io::Result<OsString> {
    let mut random = [0; NAME_LENGTH];
    random_bytes(&mut random)?;
    let count = NAME_CHARACTERS.len();
    let middle: String = random
        .iter()
        .map(|byte| char::from(NAME_CHARACTERS[usize::from(*byte) % count]))
        .collect();
    let mut name = prefix.to_os_string();
    name.push(middle);
    name.push(suffix);
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::tree;
    use std::fs;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn two_reads_of_the_source_differ() {
        let (mut first, mut second) = ([0; 16], [0; 16]);
        random_bytes(&mut first).expect("the random source reads");
        random_bytes(&mut second).expect("the random source reads");
        assert_ne!(first, second);
    }

    #[test]
    fn a_created_name_has_the_prefix_eight_name_characters_and_the_suffix() {
        let tree = tree();
        let (path, _file) = create_named(tree.path(), OsStr::new(".app.py."), ".staged")
            .expect("the file is created");
        let name = path.file_name().expect("a file name").as_bytes();
        let middle = name
            .strip_prefix(b".app.py.")
            .and_then(|rest| rest.strip_suffix(b".staged"))
            .expect("the prefix and the suffix frame the name");
        assert_eq!(middle.len(), NAME_LENGTH);
        assert!(
            middle.iter().all(|byte| NAME_CHARACTERS.contains(byte)),
            "{middle:?}"
        );
        let mode = fs::metadata(&path)
            .expect("the file exists")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, FILE_MODE);
    }

    #[test]
    fn a_missing_directory_is_the_open_error() {
        let tree = tree();
        let refused = create_named(&tree.join("missing"), OsStr::new("tmp"), "");
        assert_eq!(
            refused.map(drop).map_err(|error| error.kind()),
            Err(io::ErrorKind::NotFound)
        );
    }
}
