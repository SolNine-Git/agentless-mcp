//! What one stored row holds and how it is made: the decoded text, its digest, the analysis
//! digest of the rules, and the one parse that gives the facts and the metrics.

use std::fmt;
use std::sync::{Arc, OnceLock};

use facts::Facts;
use kernel::Key;
use metrics::{CLONE_REASONS, COMPLEXITY_REASONS, FileMetrics};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use syntax::{Language, Parsed, Unparsable};

/// The table layout of agentless.db. A database of another layout is discarded.
pub const FACTS_SCHEMA_VERSION: u16 = 1;
/// The version of the extraction rules written as code and of the blob layout. Bump it on any
/// change to what `facts::extract`, the stored reason, or a blob encoder gives for one text.
pub const FACTS_VERSION: u32 = 6;

/// One file as the caller read it: its path below the root, its language, and its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileText {
    /// The path below the repository root, with "/" between its parts.
    pub path: String,
    /// The language of the path's suffix.
    pub language: Language,
    /// The decoded text.
    pub text: Arc<str>,
}

impl FileText {
    /// Decodes `bytes` as UTF-8, with U+FFFD for each sequence that is not UTF-8.
    #[must_use]
    pub fn decode(path: String, language: Language, bytes: &[u8]) -> FileText {
        let text = Arc::from(String::from_utf8_lossy(bytes).as_ref());
        FileText {
            path,
            language,
            text,
        }
    }

    /// Returns the sha256 of the text's UTF-8 bytes, which keys the stored row.
    #[must_use]
    pub fn digest(&self) -> Key {
        text_digest(&self.text)
    }
}

// The sha256 of the UTF-8 bytes of `text`: the key of a stored row.
pub(crate) fn text_digest(text: &str) -> Key {
    Key::new(Sha256::digest(text.as_bytes()).into())
}

/// Why a file has no metrics, as a stored or computed reason text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unmeasured {
    /// The caller-facing reason, which names the file, such as "a.py does not parse: ...".
    pub reason: String,
    /// The reason in the words of the clone index, or `None` when the clone index has none.
    pub blocks_reason: Option<String>,
}

impl fmt::Display for Unmeasured {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

impl Unmeasured {
    pub(crate) fn of(error: &impl fmt::Display) -> Unmeasured {
        Unmeasured {
            reason: error.to_string(),
            blocks_reason: None,
        }
    }
}

/// The metrics of one file, or why it has none.
pub type Measured = Result<FileMetrics, Unmeasured>;

/// What one parse of one file gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Analysis {
    /// The facts.
    pub facts: Facts,
    /// The metrics, when the caller asked for them.
    pub metrics: Option<Measured>,
}

// The parse that a miss runs: syntax::parse, or a test's counting stand-in.
pub(crate) type Parse = dyn Fn(Language, Arc<str>) -> Result<Parsed, Unparsable> + Send + Sync;

/// Parses `file` once, extracts its facts, and measures it when `measure` is true.
///
/// # Errors
///
/// Returns the [`Unparsable`] of a text that the grammar does not parse.
pub fn analyze(file: &FileText, measure: bool) -> Result<Analysis, Unparsable> {
    let parsed = syntax::parse(file.language, Arc::clone(&file.text))?;
    Ok(Analysis {
        facts: facts::extract(&parsed),
        metrics: measure.then(|| measured(&parsed, &file.path)),
    })
}

// The reasons are worded when the row is made, so a stored row and a parse give the same texts. A
// language with no rules keeps the metrics error text, and the clone index has no reason for it.
pub(crate) fn measured(parsed: &Parsed, path: &str) -> Measured {
    metrics::measure(parsed).map_err(|error| Unmeasured {
        reason: error
            .reason(path, &COMPLEXITY_REASONS)
            .unwrap_or_else(|| error.to_string()),
        blocks_reason: error.reason(path, &CLONE_REASONS),
    })
}

/// Returns the digest of every rule that turns one text into its stored row. A row made under
/// another digest is never served.
#[must_use]
pub fn analysis() -> Key {
    static DIGEST: OnceLock<Key> = OnceLock::new();
    *DIGEST.get_or_init(|| Key::new(Sha256::digest(analysis_text()).into()))
}

// One line for each input: the layouts, the grammar sources, the generated language tables, and
// the metrics stamps.
pub(crate) fn analysis_text() -> String {
    let mut stamps: Vec<&str> = Language::ALL
        .into_iter()
        .filter_map(metrics::stamp)
        .collect();
    stamps.dedup();
    format!(
        "schema {FACTS_SCHEMA_VERSION}\nfacts {FACTS_VERSION}\npack {}\ntables {}\nmetrics {}\n",
        syntax::PACK_VERSION,
        syntax::TABLES_DIGEST,
        stamps.join(" ")
    )
}
