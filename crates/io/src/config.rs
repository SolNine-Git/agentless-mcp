//! The repository's `.agentless-mcp.json`: opened with the containment rules, decoded, and checked
//! key by key. Each problem is a warning, so a load never fails.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use json::{JsonValue, parse};
use serde::Serialize;
use text::CodePointString;

use crate::warning::{ConfigWarning, FileRefusal};
use crate::{byte_count, file_stays_inside, parse_config, resolve, strerror};

/// The name of the config file at the repository root.
pub const CONFIG_FILENAME: &str = ".agentless-mcp.json";
/// The most entries that `boundary_globs` can hold.
pub const MAX_BOUNDARY_GLOBS: usize = 64;
/// The most characters in one boundary glob.
pub const MAX_BOUNDARY_GLOB_CHARS: usize = 256;
// A config file is a handful of scalars, so a larger file is not a config.
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
) -> Result<Option<Vec<(CodePointString, JsonValue)>>, FileRefusal> {
    let Some(bytes) = read_bytes(path, repo_root)? else {
        return Ok(None);
    };
    let text = String::from_utf8(bytes).map_err(|_| FileRefusal::NotUtf8)?;
    let mut document = parse(&text).map_err(FileRefusal::NotJson)?;
    match &mut document {
        JsonValue::Object(entries) => Ok(Some(std::mem::take(entries))),
        _ => Err(FileRefusal::NotObject),
    }
}

// The path checks run first, so no open reaches a FIFO, a device or a file outside the root.
// O_NONBLOCK and O_NOCTTY guard a swap after the checks, and the fstat judges the opened file.
fn read_bytes(path: &Path, repo_root: &Path) -> Result<Option<Vec<u8>>, FileRefusal> {
    if !present(path, repo_root)? {
        return Ok(None);
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
        .map_err(|error| FileRefusal::Unreadable(strerror(&error)))?;
    let metadata = file
        .metadata()
        .map_err(|error| FileRefusal::Unstatable(strerror(&error)))?;
    if !metadata.is_file() {
        return Err(FileRefusal::NotRegular);
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

// Whether a file is at `path`. A path that is not a regular file inside the root, or a file over
// the cap, gives its refusal.
fn present(path: &Path, repo_root: &Path) -> Result<bool, FileRefusal> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) if path.is_symlink() => return Err(FileRefusal::NotRegular),
        Err(_) => return Ok(false),
    };
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
    Ok(true)
}

fn stays_inside(path: &Path, repo_root: &Path) -> bool {
    resolve(repo_root).is_ok_and(|root| file_stays_inside(path, &root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{Placed, build, fixture, has_mode};
    use crate::testing::Tree;
    use serde_json::{Map, Value, json};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};
    use wait_timeout::ChildExt;

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
        let warnings: Vec<String> = config.warnings.iter().map(ToString::to_string).collect();
        fields.insert("warnings".to_owned(), json!(warnings));
        Value::Object(fields)
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
                json::to_text(&config, json::JsonForm::Compact).expect("a config serializes");
            assert_eq!(tree.placed(&shown), expected, "document {text:?}");
        }
    }

    #[test]
    fn file_cases() {
        let all: Vec<(String, Vec<Value>, Value)> = fixture("config", "files");
        for (name, layout, expected) in all {
            let tree = build(&layout);
            if has_mode(&layout) && !tree.modes_bind() {
                continue;
            }
            let found = described(&load_config(&tree.join("root")), &tree);
            assert_eq!(found, expected, "case {name}");
        }
    }

    // The writer blocks in its open until a reader opens the FIFO, so its exit shows an open.
    #[cfg(target_os = "linux")]
    #[test]
    fn load_config_never_opens_a_fifo_outside_the_root() {
        let layout = [
            json!(["dir", "root"]),
            json!(["dir", "outside"]),
            json!(["fifo", "outside/pipe"]),
            json!(["link", "root/.agentless-mcp.json", "../outside/pipe"]),
        ];
        let tree = build(&layout);
        let mut writer = Command::new("sh")
            .args(["-c", "exec 3>\"$0\""])
            .arg(tree.join("outside/pipe"))
            .stdin(Stdio::null())
            .spawn()
            .expect("sh starts");
        let blocked = blocked_in_open(writer.id());
        let config = load_config(&tree.join("root"));
        let released = writer.wait_timeout(Duration::from_millis(500));
        writer.kill().expect("the writer is killed");
        writer.wait().expect("the writer is reaped");
        assert!(blocked, "the writer did not reach its open within the wait");
        assert_eq!(
            config.warnings,
            [ConfigWarning::File(FileRefusal::NotRegular)]
        );
        let released = released.expect("the wait for the writer");
        assert_eq!(
            released, None,
            "the load opened the FIFO and released the writer"
        );
    }

    // The kernel names the wait of a FIFO open for a missing partner in the task's wchan.
    #[cfg(target_os = "linux")]
    fn blocked_in_open(pid: u32) -> bool {
        let wchan = format!("/proc/{pid}/wchan");
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if fs::read_to_string(&wchan).is_ok_and(|text| text == "wait_for_partner") {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
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
