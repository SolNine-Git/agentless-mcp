//! The key-by-key check of a decoded config object.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use pycompat::{JsonValue, PyStr, is_printable, is_space, strip};

use crate::config::{
    GateMode, GatedCheck, Granularity, KNOWN_KEYS, MAX_BOUNDARY_GLOB_CHARS, MAX_BOUNDARY_GLOBS,
    ProjectConfig,
};
use crate::warning::{
    ConfigWarning, GateWarning, GlobsWarning, StoplistWarning, TestCmdWarning, UnknownKeyWarning,
    ValueWarning,
};

pub(crate) const MAX_STOPLIST_ENTRIES: usize = 200;
pub(crate) const MAX_STOPLIST_ENTRY_CHARS: usize = 64;
pub(crate) const MAX_TEST_CMD_CHARS: usize = 512;
const MAX_UNKNOWN_KEY_WARNINGS: usize = 8;
const FORBIDDEN_NAME_CHARS: &str = "/\\:;'\"`$*?<>|\0";

struct IntKey {
    name: &'static str,
    low: u32,
    high: u32,
}

const MAP_BUDGET: IntKey = IntKey {
    name: "map_budget",
    low: 200,
    high: 64_000,
};
const MAX_FILES: IntKey = IntKey {
    name: "max_files",
    low: 1,
    high: 200,
};
const COMPLEXITY_THRESHOLD: IntKey = IntKey {
    name: "complexity_threshold",
    low: 1,
    high: 100,
};

type Document = [(PyStr, JsonValue)];

/// Checks each key of a decoded config object. A bad value gives a warning and no value; the
/// value warnings come first, in key order, and the unknown keys after them.
#[must_use]
pub fn parse_config(document: &Document, path: Option<PathBuf>) -> ProjectConfig {
    let mut warnings = Vec::new();
    let config = ProjectConfig {
        path,
        map_budget: bounded_int(document, &MAP_BUDGET, &mut warnings),
        max_files: bounded_int(document, &MAX_FILES, &mut warnings),
        granularity: granularity(document, &mut warnings),
        docstrings: boolean(document, "docstrings", &mut warnings),
        stoplist: stoplist(document, &mut warnings),
        test_cmd: test_cmd(document, &mut warnings),
        relation_weights: boolean(document, "relation_weights", &mut warnings),
        complexity_threshold: bounded_int(document, &COMPLEXITY_THRESHOLD, &mut warnings),
        erosion_gate: erosion_gate(document, &mut warnings),
        boundary_globs: boundary_globs(document, &mut warnings),
        warnings: Vec::new(),
    };
    unknown_keys(document, &mut warnings);
    ProjectConfig { warnings, ..config }
}

fn lookup<'a>(document: &'a Document, key: &str) -> Option<&'a JsonValue> {
    document
        .iter()
        .find(|(name, _)| name.as_str() == Some(key))
        .map(|(_, value)| value)
}

// A string with a lone surrogate has no text form, so it counts as no string.
fn text_of(value: &JsonValue) -> Option<&str> {
    match value {
        JsonValue::Str(text) => text.as_str(),
        _ => None,
    }
}

fn bounded_int(
    document: &Document,
    key: &IntKey,
    warnings: &mut Vec<ConfigWarning>,
) -> Option<u32> {
    let value = lookup(document, key.name)?;
    let JsonValue::Int(digits) = value else {
        warnings.push(ConfigWarning::Value(ValueWarning::NotInteger {
            key: key.name,
        }));
        return None;
    };
    let in_range = digits
        .parse::<u32>()
        .ok()
        .filter(|number| (key.low..=key.high).contains(number));
    if in_range.is_none() {
        let (key, low, high) = (key.name, key.low, key.high);
        warnings.push(ConfigWarning::Value(ValueWarning::OutOfRange {
            key,
            low,
            high,
        }));
    }
    in_range
}

fn granularity(document: &Document, warnings: &mut Vec<ConfigWarning>) -> Option<Granularity> {
    let value = lookup(document, "granularity")?;
    let text = text_of(value);
    let found = Granularity::ALL
        .into_iter()
        .find(|choice| text == Some(choice.name()));
    if found.is_none() {
        let key = "granularity";
        let choices = "function, file";
        warnings.push(ConfigWarning::Value(ValueWarning::NotChoice {
            key,
            choices,
        }));
    }
    found
}

fn boolean(
    document: &Document,
    key: &'static str,
    warnings: &mut Vec<ConfigWarning>,
) -> Option<bool> {
    let value = lookup(document, key)?;
    if let JsonValue::Bool(flag) = value {
        return Some(*flag);
    }
    warnings.push(ConfigWarning::Value(ValueWarning::NotBoolean { key }));
    None
}

fn stoplist(document: &Document, warnings: &mut Vec<ConfigWarning>) -> BTreeSet<String> {
    let Some(value) = lookup(document, "stoplist") else {
        return BTreeSet::new();
    };
    let JsonValue::Array(entries) = value else {
        warnings.push(ConfigWarning::Stoplist(StoplistWarning::NotList));
        return BTreeSet::new();
    };
    if entries.len() > MAX_STOPLIST_ENTRIES {
        let count = entries.len();
        warnings.push(ConfigWarning::Stoplist(StoplistWarning::OverCap { count }));
    }
    let used = &entries[..entries.len().min(MAX_STOPLIST_ENTRIES)];
    let names: BTreeSet<String> = used
        .iter()
        .filter_map(bare_name)
        .map(str::to_owned)
        .collect();
    let count = used
        .iter()
        .filter(|entry| bare_name(entry).is_none())
        .count();
    if count > 0 {
        warnings.push(ConfigWarning::Stoplist(StoplistWarning::Dropped { count }));
    }
    names
}

fn bare_name(entry: &JsonValue) -> Option<&str> {
    let name = text_of(entry)?;
    let length = name.chars().count();
    let clean = !name
        .chars()
        .any(|c| is_space(c) || FORBIDDEN_NAME_CHARS.contains(c));
    (length > 0 && length <= MAX_STOPLIST_ENTRY_CHARS && clean).then_some(name)
}

fn test_cmd(document: &Document, warnings: &mut Vec<ConfigWarning>) -> Option<String> {
    let value = lookup(document, "test_cmd")?;
    let checked = match text_of(value) {
        Some(command) if !strip(command).is_empty() => single_line(command),
        _ => Err(TestCmdWarning::Empty),
    };
    match checked {
        Ok(command) => Some(command.to_owned()),
        Err(warning) => {
            warnings.push(ConfigWarning::TestCmd(warning));
            None
        }
    }
}

fn single_line(command: &str) -> Result<&str, TestCmdWarning> {
    if command.chars().count() > MAX_TEST_CMD_CHARS {
        return Err(TestCmdWarning::TooLong);
    }
    if command.contains(['\n', '\r']) {
        return Err(TestCmdWarning::Multiline);
    }
    Ok(command)
}

fn erosion_gate(
    document: &Document,
    warnings: &mut Vec<ConfigWarning>,
) -> Option<BTreeMap<GatedCheck, GateMode>> {
    let value = lookup(document, "erosion_gate")?;
    let JsonValue::Object(entries) = value else {
        warnings.push(ConfigWarning::ErosionGate(GateWarning::NotObject));
        return None;
    };
    let mut modes = BTreeMap::new();
    for check in GatedCheck::ALL {
        let Some(mode) = lookup(entries, check.name()) else {
            continue;
        };
        let text = text_of(mode);
        match GateMode::ALL
            .into_iter()
            .find(|known| text == Some(known.name()))
        {
            Some(mode) => {
                modes.insert(check, mode);
            }
            None => warnings.push(ConfigWarning::ErosionGate(GateWarning::Mode(check))),
        }
    }
    let count = entries.iter().filter(|(key, _)| !is_gated(key)).count();
    if count > 0 {
        warnings.push(ConfigWarning::ErosionGate(GateWarning::Unknown { count }));
    }
    Some(modes)
}

fn is_gated(key: &PyStr) -> bool {
    GatedCheck::ALL
        .iter()
        .any(|check| key.as_str() == Some(check.name()))
}

fn boundary_globs(document: &Document, warnings: &mut Vec<ConfigWarning>) -> Vec<String> {
    let Some(value) = lookup(document, "boundary_globs") else {
        return Vec::new();
    };
    let JsonValue::Array(entries) = value else {
        warnings.push(ConfigWarning::BoundaryGlobs(GlobsWarning::NotList));
        return Vec::new();
    };
    if entries.len() > MAX_BOUNDARY_GLOBS {
        let count = entries.len();
        warnings.push(ConfigWarning::BoundaryGlobs(GlobsWarning::TooMany {
            count,
        }));
        return Vec::new();
    }
    let globs: Vec<String> = entries
        .iter()
        .filter_map(relative_glob)
        .map(str::to_owned)
        .collect();
    if globs.len() < entries.len() {
        let count = entries.len() - globs.len();
        warnings.push(ConfigWarning::BoundaryGlobs(GlobsWarning::Dropped {
            count,
        }));
    }
    globs
}

// A lone surrogate is not printable, so a glob with one is dropped before it needs a text form.
fn relative_glob(entry: &JsonValue) -> Option<&str> {
    let JsonValue::Str(text) = entry else {
        return None;
    };
    let length = text.code_points().count();
    let printable = text.code_points().all(is_printable);
    let glob = text.as_str()?;
    let fits = length > 0 && length <= MAX_BOUNDARY_GLOB_CHARS && printable;
    let relative = !glob.starts_with('/') && !glob.contains('\\');
    let plain = glob.split('/').all(|part| !matches!(part, "" | "." | ".."));
    (fits && relative && plain).then_some(glob)
}

fn unknown_keys(document: &Document, warnings: &mut Vec<ConfigWarning>) {
    let mut unknown: Vec<&PyStr> = document
        .iter()
        .map(|(key, _)| key)
        .filter(|key| !KNOWN_KEYS.iter().any(|known| key.as_str() == Some(known)))
        .collect();
    unknown.sort();
    let listed = unknown.iter().take(MAX_UNKNOWN_KEY_WARNINGS);
    warnings
        .extend(listed.map(|key| ConfigWarning::Unknown(UnknownKeyWarning::Key((*key).clone()))));
    if unknown.len() > MAX_UNKNOWN_KEY_WARNINGS {
        let count = unknown.len() - MAX_UNKNOWN_KEY_WARNINGS;
        warnings.push(ConfigWarning::Unknown(UnknownKeyWarning::Suppressed {
            count,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pycompat::loads;

    fn parsed(text: &str) -> ProjectConfig {
        let mut document = loads(text).expect("the test document is JSON");
        let JsonValue::Object(entries) = &mut document else {
            panic!("the test document is an object");
        };
        parse_config(entries, None)
    }

    #[test]
    fn a_lone_surrogate_in_a_stoplist_name_drops_the_name() {
        let config = parsed(r#"{"stoplist": ["\udcff", "ok"]}"#);
        assert_eq!(config.stoplist, BTreeSet::from(["ok".to_owned()]));
        let dropped = ConfigWarning::Stoplist(StoplistWarning::Dropped { count: 1 });
        assert_eq!(config.warnings, [dropped]);
    }

    #[test]
    fn a_lone_surrogate_in_the_test_command_ignores_the_command() {
        let config = parsed(r#"{"test_cmd": "make \udcff"}"#);
        assert_eq!(config.test_cmd, None);
        assert_eq!(
            config.warnings,
            [ConfigWarning::TestCmd(TestCmdWarning::Empty)]
        );
    }
}
