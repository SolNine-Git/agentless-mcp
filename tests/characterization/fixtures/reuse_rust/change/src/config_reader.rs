use std::fs;
use std::path::Path;

use crate::settings::validate_keys;

pub fn read_config(location: &Path) -> Result<Settings, Error> {
    let text = fs::read_to_string(location)?;
    let mut loaded: Settings = toml::from_str(&text)?;
    loaded.jobs = loaded.jobs.max(1);
    validate_keys(&loaded)?;
    Ok(loaded)
}
