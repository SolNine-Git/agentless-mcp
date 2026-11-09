use std::path::PathBuf;

pub fn common_root(paths: &[PathBuf]) -> Option<PathBuf> {
    let first = paths.first()?;
    first
        .ancestors()
        .find(|root| paths.iter().all(|path| path.starts_with(root)))
        .map(|root| root.to_path_buf())
}
