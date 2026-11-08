use std::path::PathBuf;

pub fn shared_parent(files: &[PathBuf]) -> Option<PathBuf> {
    let mut found = None;
    for candidate in files.first()?.ancestors() {
        if files.iter().all(|file| file.starts_with(candidate)) {
            found = Some(candidate.to_path_buf());
            break;
        }
    }
    found
}
