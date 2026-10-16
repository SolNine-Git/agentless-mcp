//! The dependency manifests at the repository root, read for `undeclared_imports`.
//!
//! A manifest that cannot be read or parsed contributes a warning and nothing else: an empty
//! declared set would make every third-party import look undeclared.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use patchcheck::{
    DeclaredDependencies, PYPROJECT_NAME, REQUIREMENTS_GLOB, parse_pyproject_dependencies,
    parse_requirements,
};
use text::CodePointString;

use crate::rows::bounded_read;

// A repository is cloned from strangers, so a manifest symlinked out of it is not its manifest.
const MANIFEST_OUTSIDE_ROOT: &str = "refused: a symlink whose target is outside the repository";

// pyproject.toml, then each requirements*.txt file in code point order of the names.
pub(super) fn read_declared_dependencies(root: &Path) -> DeclaredDependencies {
    let mut declared = DeclaredDependencies::default();
    let resolved = io::resolve(root).unwrap_or_else(|_| root.to_path_buf());
    let pyproject = root.join(PYPROJECT_NAME);
    if pyproject.is_file() {
        read_pyproject(&mut declared, &pyproject, &resolved);
    }
    for (path, name) in requirements_files(root) {
        if path.is_file() {
            read_requirements(&mut declared, &path, &name, &resolved);
        }
    }
    declared
}

fn read_pyproject(declared: &mut DeclaredDependencies, path: &Path, root: &Path) {
    let text = match manifest_text(path, root) {
        Ok(text) => text,
        Err(skipped) => {
            let warning = format!("{PYPROJECT_NAME} not read: {skipped}");
            declared.warnings.push(warning);
            return;
        }
    };
    let parse = parse_pyproject_dependencies(&text);
    declared.packages.extend(parse.packages);
    declared.warnings.extend(parse.warnings);
    declared.requires_python = parse.requires_python;
    if parse.parsed {
        declared.sources.push(PYPROJECT_NAME.to_owned());
    }
}

fn read_requirements(declared: &mut DeclaredDependencies, path: &Path, name: &str, root: &Path) {
    match manifest_text(path, root) {
        Ok(text) => {
            declared.packages.extend(parse_requirements(&text));
            declared.sources.push(name.to_owned());
        }
        Err(skipped) => declared
            .warnings
            .push(format!("{name} not read: {skipped}")),
    }
}

// The bounded read, decoded with U+FFFD, of a manifest that stays inside `root`.
fn manifest_text(path: &Path, root: &Path) -> Result<String, String> {
    if !io::file_stays_inside(path, root) {
        return Err(MANIFEST_OUTSIDE_ROOT.to_owned());
    }
    let bytes = bounded_read(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

// The entries of `root` that the glob requirements*.txt matches; a root that cannot be listed
// has none.
fn requirements_files(root: &Path) -> Vec<(PathBuf, String)> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found: Vec<(CodePointString, PathBuf, String)> = entries
        .filter_map(Result::ok)
        .filter(|entry| requirements_name(&entry.file_name()))
        .map(|entry| {
            let name = entry.file_name();
            let key = CodePointString::from_utf8_escaped(name.as_bytes());
            (key, entry.path(), name.to_string_lossy().into_owned())
        })
        .collect();
    found.sort_by(|left, right| left.0.cmp(&right.0));
    found
        .into_iter()
        .map(|(_, path, name)| (path, name))
        .collect()
}

// The glob's one star matches any run of characters, an empty one too.
fn requirements_name(name: &OsStr) -> bool {
    let (prefix, suffix) = REQUIREMENTS_GLOB
        .split_once('*')
        .expect("the requirements glob holds one star");
    let bytes = name.as_bytes();
    bytes.len() >= prefix.len() + suffix.len()
        && bytes.starts_with(prefix.as_bytes())
        && bytes.ends_with(suffix.as_bytes())
}
