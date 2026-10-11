use std::path::{Path, PathBuf};
use std::process::Command;

use serde::de::DeserializeOwned;

use crate::Failure;

/// Return the repository root: the directory above this crate, as `cargo run` reports it.
pub fn root() -> Result<PathBuf, Failure> {
    // Read at run time: a compiled-in path goes stale when a shared target dir serves a moved tree.
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR")
        .ok_or_else(|| Failure::Broken("run this through cargo xtask".to_owned()))?;
    let above = Path::new(&manifest_dir).join("..");
    std::fs::canonicalize(&above)
        .map_err(|error| Failure::Broken(format!("{}: {error}", above.display())))
}

/// Return `path` relative to `root`, with forward slashes.
pub fn relative(path: &Path, root: &Path) -> String {
    let inside = path.strip_prefix(root).unwrap_or(path);
    inside.to_string_lossy().replace('\\', "/")
}

/// Read a file as text, naming it in the failure.
pub fn read(path: &Path) -> Result<String, Failure> {
    std::fs::read_to_string(path).map_err(|error| broken(path, &error))
}

/// Write a file, naming it in the failure.
pub fn write(path: &Path, text: &str) -> Result<(), Failure> {
    std::fs::write(path, text).map_err(|error| broken(path, &error))
}

/// Parse a TOML file, naming it in the failure.
pub fn toml<T: DeserializeOwned>(path: &Path) -> Result<T, Failure> {
    toml::from_str(&read(path)?).map_err(|error| broken(path, &error))
}

/// Return the directories under `crates/` that hold a `Cargo.toml`, sorted.
pub fn crate_dirs(root: &Path) -> Result<Vec<PathBuf>, Failure> {
    let crates = root.join("crates");
    let entries = std::fs::read_dir(&crates).map_err(|error| broken(&crates, &error))?;
    let mut dirs = Vec::new();
    for entry in entries {
        let path = entry.map_err(|error| broken(&crates, &error))?.path();
        if path.join("Cargo.toml").is_file() {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// Return every `.rs` file under `dir`, sorted.
pub fn rust_files(dir: &Path) -> Result<Vec<PathBuf>, Failure> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let entries = std::fs::read_dir(&current).map_err(|error| broken(&current, &error))?;
        for entry in entries {
            let path = entry.map_err(|error| broken(&current, &error))?.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Run `command` to completion and return its stdout; a non-zero exit is a failure.
pub fn stdout(command: &mut Command) -> Result<Vec<u8>, Failure> {
    let output = command
        .output()
        .map_err(|error| Failure::Broken(format!("{command:?}: {error}")))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(Failure::Broken(format!(
            "{command:?} exited with {}: {}",
            output.status,
            stderr.trim()
        )))
    }
}

fn broken(path: &Path, error: &dyn std::fmt::Display) -> Failure {
    Failure::Broken(format!("{}: {error}", path.display()))
}
