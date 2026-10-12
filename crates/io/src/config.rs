//! The repository's `.agentless-mcp.json`: opened with the containment rules, decoded, and checked
//! key by key. Each problem is a warning, so a load never fails.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use pycompat::{JsonValue, PyStr, loads};
use serde::Serialize;

use crate::warning::{ConfigWarning, FileRefusal};
use crate::{byte_count, file_stays_inside, parse_config, resolve, strerror};

/// The name of the config file at the repository root.
pub const CONFIG_FILENAME: &str = ".agentless-mcp.json";
/// The most entries that `boundary_globs` can hold.
pub const MAX_BOUNDARY_GLOBS: usize = 64;
/// The most characters in one boundary glob.
pub const MAX_BOUNDARY_GLOB_CHARS: usize = 256;
pub(crate) const MAX_CONFIG_BYTES: u64 = 65_536;
// In the order that the unknown-key warning lists them.
pub(crate) const KNOWN_KEYS: [&str; 10] = [
    "map_budget",
    "max_files",
    "granularity",
    "docstrings",
    "stoplist",
    "test_cmd",
    "relation_weights",
    "complexity_threshold",
    "erosion_gate",
    "boundary_globs",
];

/// The checked values of one config file. A field without a value is `None` or empty. It
/// serializes its fields in order, apart from `warnings`, which the caller renders.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ProjectConfig {
    /// The config file that was read, or `None` when no file was used.
    pub path: Option<PathBuf>,
    /// The `map_budget` value.
    pub map_budget: Option<u32>,
    /// The `max_files` value.
    pub max_files: Option<u32>,
    /// The `granularity` value.
    pub granularity: Option<Granularity>,
    /// The `docstrings` value.
    pub docstrings: Option<bool>,
    /// The `stoplist` names.
    pub stoplist: BTreeSet<String>,
    /// The `test_cmd` value.
    pub test_cmd: Option<String>,
    /// The `relation_weights` value.
    pub relation_weights: Option<bool>,
    /// The `complexity_threshold` value.
    pub complexity_threshold: Option<u32>,
    /// The mode of each gated check that `erosion_gate` names, or `None` without the key.
    pub erosion_gate: Option<BTreeMap<GatedCheck, GateMode>>,
    /// The `boundary_globs` values, in file order.
    pub boundary_globs: Vec<String>,
    /// Each problem, in the order that the load found it.
    // Two warning texts are prompts texts, so only the caller can render the list.
    #[serde(skip)]
    pub warnings: Vec<ConfigWarning>,
}

/// The map granularity that a config can set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    /// The symbols of each ranked file.
    Function,
    /// The ranked files alone.
    File,
}

impl Granularity {
    /// Every granularity, in the order of the warning text.
    pub const ALL: [Self; 2] = [Self::Function, Self::File];

    /// Returns the value as the config spells it.
    #[must_use]
    pub fn name(self) -> &'static str {
        ["function", "file"][self as usize]
    }
}

/// A check that the erosion gate hook gates. The order is the config order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GatedCheck {
    /// `complexity_delta`.
    ComplexityDelta,
    /// `contract_drift`.
    ContractDrift,
    /// `clone_delta`.
    CloneDelta,
}

impl GatedCheck {
    /// Every gated check, in the config order.
    pub const ALL: [Self; 3] = [Self::ComplexityDelta, Self::ContractDrift, Self::CloneDelta];

    /// Returns the check as the config spells it.
    #[must_use]
    pub fn name(self) -> &'static str {
        ["complexity_delta", "contract_drift", "clone_delta"][self as usize]
    }
}

/// The hook's mode for one gated check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GateMode {
    /// The hook does not run the check.
    Off,
    /// The hook reports a finding and allows the edit.
    Advisory,
    /// The hook refuses an edit with a finding.
    Block,
}

impl GateMode {
    /// Every mode, in the order of the warning text.
    pub const ALL: [Self; 3] = [Self::Off, Self::Advisory, Self::Block];

    /// Returns the mode as the config spells it.
    #[must_use]
    pub fn name(self) -> &'static str {
        ["off", "advisory", "block"][self as usize]
    }
}

/// Reads `repo_root/.agentless-mcp.json` without blocking on a FIFO. A missing file gives the
/// empty config, and each other problem gives the empty config with one warning.
#[must_use]
pub fn load_config(repo_root: &Path) -> ProjectConfig {
    let path = repo_root.join(CONFIG_FILENAME);
    match read_object(&path, repo_root) {
        Ok(Some(document)) => parse_config(&document, Some(path)),
        Ok(None) => ProjectConfig::default(),
        Err(refusal) => ProjectConfig {
            warnings: vec![ConfigWarning::File(refusal)],
            ..ProjectConfig::default()
        },
    }
}

fn read_object(
    path: &Path,
    repo_root: &Path,
) -> Result<Option<Vec<(PyStr, JsonValue)>>, FileRefusal> {
    let Some(bytes) = read_bytes(path, repo_root)? else {
        return Ok(None);
    };
    let text = String::from_utf8(bytes).map_err(|_| FileRefusal::NotUtf8)?;
    let mut document = loads(&text).map_err(FileRefusal::NotJson)?;
    match &mut document {
        JsonValue::Object(entries) => Ok(Some(std::mem::take(entries))),
        _ => Err(FileRefusal::NotObject),
    }
}

// O_NONBLOCK keeps a FIFO from stalling the open, and the stat judges the file that was opened.
fn read_bytes(path: &Path, repo_root: &Path) -> Result<Option<Vec<u8>>, FileRefusal> {
    let opened = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path);
    let file = match opened {
        Ok(file) => file,
        Err(error) => return open_failure(path, repo_root, &error),
    };
    let metadata = file
        .metadata()
        .map_err(|error| FileRefusal::Unstatable(strerror(&error)))?;
    if !metadata.is_file() {
        return Err(FileRefusal::NotRegular);
    }
    if !stays_inside(path, repo_root) {
        return Err(FileRefusal::Outside);
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(FileRefusal::TooLarge {
            size: metadata.len(),
        });
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| FileRefusal::Unreadable(strerror(&error)))?;
    if byte_count(&bytes) > MAX_CONFIG_BYTES {
        return Err(FileRefusal::Grew);
    }
    Ok(Some(bytes))
}

// Names a failed open by what is at the path: nothing, something that is not a regular file, a
// file outside the root, or a file that cannot be read.
fn open_failure(
    path: &Path,
    repo_root: &Path,
    error: &io::Error,
) -> Result<Option<Vec<u8>>, FileRefusal> {
    match fs::metadata(path) {
        Ok(metadata) if !metadata.is_file() => Err(FileRefusal::NotRegular),
        Ok(_) if !stays_inside(path, repo_root) => Err(FileRefusal::Outside),
        Ok(_) => Err(FileRefusal::Unreadable(strerror(error))),
        Err(_) if path.is_symlink() => Err(FileRefusal::NotRegular),
        Err(_) => Ok(None),
    }
}

fn stays_inside(path: &Path, repo_root: &Path) -> bool {
    resolve(repo_root).is_ok_and(|root| file_stays_inside(path, &root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{Placed, build, fixture};
    use crate::testing::Tree;
    use crate::warning::{GateWarning, GlobsWarning};
    use serde_json::{Map, Value, json};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    fn described(config: &ProjectConfig, tree: &Tree) -> Value {
        let mut fields = Map::new();
        let path = config
            .path
            .as_ref()
            .map(|path| tree.placed(&path.to_string_lossy()));
        fields.insert("path".to_owned(), json!(path));
        fields.insert("map_budget".to_owned(), json!(config.map_budget));
        fields.insert("max_files".to_owned(), json!(config.max_files));
        fields.insert(
            "granularity".to_owned(),
            json!(config.granularity.map(Granularity::name)),
        );
        fields.insert("docstrings".to_owned(), json!(config.docstrings));
        fields.insert("stoplist".to_owned(), json!(config.stoplist));
        fields.insert("test_cmd".to_owned(), json!(config.test_cmd));
        fields.insert(
            "relation_weights".to_owned(),
            json!(config.relation_weights),
        );
        fields.insert(
            "complexity_threshold".to_owned(),
            json!(config.complexity_threshold),
        );
        let gate = config.erosion_gate.as_ref().map(|modes| {
            let pairs = modes
                .iter()
                .map(|(check, mode)| (check.name().to_owned(), json!(mode.name())));
            Value::Object(pairs.collect())
        });
        fields.insert("erosion_gate".to_owned(), json!(gate));
        fields.insert("boundary_globs".to_owned(), json!(config.boundary_globs));
        let warnings: Vec<Value> = config.warnings.iter().map(typed).collect();
        fields.insert("warnings".to_owned(), json!(warnings));
        Value::Object(fields)
    }

    fn typed(warning: &ConfigWarning) -> Value {
        match warning {
            ConfigWarning::File(refusal) => json!(["text", refusal.to_string()]),
            ConfigWarning::Value(value) => json!(["text", value.to_string()]),
            ConfigWarning::Stoplist(stoplist) => json!(["text", stoplist.to_string()]),
            ConfigWarning::TestCmd(test_cmd) => json!(["text", test_cmd.to_string()]),
            ConfigWarning::Unknown(unknown) => json!(["text", unknown.to_string()]),
            ConfigWarning::ErosionGate(gate) => typed_gate(*gate),
            ConfigWarning::BoundaryGlobs(globs) => typed_globs(*globs),
        }
    }

    fn typed_gate(warning: GateWarning) -> Value {
        match warning {
            GateWarning::NotObject => json!(["erosion_gate_not_object"]),
            GateWarning::Mode(check) => json!(["erosion_gate_mode", check.name()]),
            GateWarning::Unknown { count } => json!(["erosion_gate_unknown", count]),
        }
    }

    fn typed_globs(warning: GlobsWarning) -> Value {
        match warning {
            GlobsWarning::NotList => json!(["boundary_globs_not_list"]),
            GlobsWarning::TooMany { count } => json!(["boundary_globs_too_many", count]),
            GlobsWarning::Dropped { count } => json!(["boundary_globs_dropped", count]),
        }
    }

    // Loads `text` as the config file of a new tree.
    fn loaded_document(text: &str) -> (Tree, ProjectConfig) {
        let layout = [
            json!(["dir", "root"]),
            json!(["file", "root/.agentless-mcp.json", text]),
        ];
        let tree = build(&layout);
        let config = load_config(&tree.join("root"));
        (tree, config)
    }

    #[test]
    fn document_cases() {
        let all: Vec<(String, Value)> = fixture("config", "documents");
        for (text, expected) in all {
            let (tree, config) = loaded_document(&text);
            assert_eq!(described(&config, &tree), expected, "document {text:?}");
        }
    }

    #[test]
    fn serialized_cases() {
        let all: Vec<(String, String)> = fixture("config", "serialized");
        for (text, expected) in all {
            let (tree, config) = loaded_document(&text);
            let shown =
                pycompat::dumps(&config, pycompat::JsonForm::Compact).expect("a config serializes");
            assert_eq!(tree.placed(&shown), expected, "document {text:?}");
        }
    }

    #[test]
    fn file_cases() {
        let all: Vec<(String, Vec<Value>, Value)> = fixture("config", "files");
        for (name, layout, expected) in all {
            let tree = build(&layout);
            let found = described(&load_config(&tree.join("root")), &tree);
            assert_eq!(found, expected, "case {name}");
        }
    }

    #[test]
    fn a_fifo_at_the_config_path_does_not_block_the_load() {
        let layout = [
            json!(["dir", "root"]),
            json!(["fifo", "root/.agentless-mcp.json"]),
        ];
        let tree = build(&layout);
        let root = tree.join("root");
        let (sender, loaded) = mpsc::channel();
        thread::spawn(move || sender.send(load_config(&root)).is_ok());
        let config = loaded
            .recv_timeout(Duration::from_secs(10))
            .expect("the load returns without a writer on the FIFO");
        assert_eq!(
            config.warnings,
            [ConfigWarning::File(FileRefusal::NotRegular)]
        );
    }

    #[test]
    fn a_file_that_grew_during_the_read_displays_its_warning() {
        let text =
            ".agentless-mcp.json grew past the 65536-byte cap while it was being read; ignored";
        assert_eq!(FileRefusal::Grew.to_string(), text);
    }
}
