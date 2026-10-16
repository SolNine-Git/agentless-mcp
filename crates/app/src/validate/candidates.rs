//! The candidate set: one file per candidate patch, its stem the id, the sorted order of the
//! directory the first-appearance order.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::{MAX_CANDIDATE_BYTES, MAX_CANDIDATES};
use crate::lint::{name, patch_files, stem};
use crate::repo::{below_cwd, expand_user};
use crate::{Error, Services};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) id: String,
    pub(crate) index: usize,
    pub(crate) text: String,
}

// Two files that share a stem are refused: their ids would collide in the report. The two caps
// keep a --candidates that names a source tree from becoming hours of test runs.
pub(crate) fn load_candidates(
    services: &Services,
    directory: &Path,
) -> Result<Vec<Candidate>, Error> {
    let expanded = expand_user(directory, services.home.as_deref());
    let resolved = io::resolve(&below_cwd(services, &expanded)?)
        .map_err(|error| unreadable(io::strerror(&error)))?;
    if !resolved.is_dir() {
        let shown = resolved.display();
        return Err(unreadable(format!(
            "--candidates must name a directory; {shown} is not one"
        )));
    }
    let files = patch_files(&resolved)?;
    if files.is_empty() {
        let shown = resolved.display();
        return Err(failed(format!(
            "no candidate files in {shown}: one file per candidate patch"
        )));
    }
    if files.len() > MAX_CANDIDATES {
        return Err(failed(format!(
            "{} holds {} files and a run takes at most {MAX_CANDIDATES}. One file is one \
             candidate patch, so this directory is probably not a candidate set.",
            resolved.display(),
            files.len()
        )));
    }
    read_all(&files)
}

// The stem check comes before each file's read, so a collision is refused before its file is.
fn read_all(files: &[PathBuf]) -> Result<Vec<Candidate>, Error> {
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut candidates = Vec::with_capacity(files.len());
    for (index, path) in files.iter().enumerate() {
        let id = stem(path);
        if let Some(first) = seen.get(&id) {
            return Err(failed(format!(
                "two candidates share the id {}: {first} and {}. Candidate ids come from the \
                 filename stem and must be unique.",
                text::CodePointString::from(id.as_str()).quoted(),
                name(path)
            )));
        }
        seen.insert(id.clone(), name(path));
        let text = read_candidate(path)?;
        candidates.push(Candidate { id, index, text });
    }
    Ok(candidates)
}

// The text as a text-mode read gives it: strict UTF-8, each CRLF and lone CR a line feed.
fn read_candidate(path: &Path) -> Result<String, Error> {
    let shown = name(path);
    let cannot_read = |error: &std::io::Error| {
        unreadable(format!(
            "cannot read candidate {shown}: {}",
            io::strerror(error)
        ))
    };
    let size = fs::metadata(path)
        .map_err(|error| cannot_read(&error))?
        .len();
    if size > MAX_CANDIDATE_BYTES as u64 {
        return Err(too_large(&shown, size));
    }
    let bytes = io::read_bounded(path).map_err(|skipped| match skipped {
        io::Skipped::TooLarge { size } => too_large(&shown, size),
        io::Skipped::Unreadable(error) => cannot_read(&error),
    })?;
    let text = io::decode_strict(bytes)
        .map_err(|_| unreadable(format!("candidate {shown} is not UTF-8 text")))?;
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

fn too_large(shown: &str, size: u64) -> Error {
    failed(format!(
        "candidate {shown} is {size} bytes and the limit is {MAX_CANDIDATE_BYTES}. A patch \
         larger than that is not a patch."
    ))
}

fn unreadable(message: String) -> Error {
    kernel::Error::InputUnreadable(message).into()
}

fn failed(message: String) -> Error {
    kernel::Error::OperationFailed(message).into()
}
