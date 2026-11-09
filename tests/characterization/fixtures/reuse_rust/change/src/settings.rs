use std::fs;
use std::path::Path;

pub fn load_settings(path: &Path) -> Result<Settings, Error> {
    let raw = fs::read_to_string(path)?;
    let mut data: Settings = toml::from_str(&raw)?;
    data.jobs = data.jobs.max(1);
    validate_keys(&data)?;
    Ok(data)
}

pub fn validate_keys(data: &Settings) -> Result<(), Error> {
    if data.root.as_os_str().is_empty() {
        return Err(Error::NoRoot);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_job_count() {
        let raw = fs::read_to_string("settings.toml").expect("a file");
        let mut data: Settings = toml::from_str(&raw).expect("settings");
        data.jobs = data.jobs.max(1);
        validate_keys(&data).expect("valid");
        assert_eq!(data.jobs, 1);
    }
}
