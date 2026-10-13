//! The index plan: whether each walked file is reused, extracted, skipped or failed, decided
//! before any parse, and the join of the parse results to the plan.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use kernel::Key;
use syntax::{Language, Unparsable};

use crate::{Encoded, EvictionReport, FileText, IndexFailure, IndexReport, IndexSkip, RowCounts};

/// One walked file of a known language, as the caller observed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observed {
    /// The path below the repository root.
    pub path: String,
    /// The language of the path's suffix.
    pub language: Language,
    /// True when the file gets metrics: it is in the metric scope and its language has rules.
    pub measure: bool,
    /// What the read of the file found.
    pub read: Read,
}

/// What the read of one walked file found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Read {
    /// The walk's size is over the per-file cap, so the file was not read.
    Oversize {
        /// The size in bytes.
        size: u64,
    },
    /// The read failed, for the reason given.
    Unreadable {
        /// The reason, such as "unreadable: Permission denied".
        reason: String,
    },
    /// The decoded text.
    Text(FileText),
}

/// What a database held before the run: each path's row key and the analysis of its meta row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Previous {
    pub(crate) entries: BTreeMap<String, Entry>,
    pub(crate) analysis: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) language: String,
    pub(crate) digest: Vec<u8>,
    pub(crate) measured: bool,
}

/// One file to parse, with whether to measure it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extract {
    /// The file.
    pub file: FileText,
    /// True when the parse also measures the file.
    pub measure: bool,
    digest: Key,
}

/// The decisions of one run before any parse. The caller parses `extract` and settles.
#[derive(Debug, Default)]
pub struct Plan {
    /// The files to parse, in walk order.
    pub extract: Vec<Extract>,
    // The walk position of each file in `extract`.
    positions: Vec<usize>,
    seen: BTreeSet<String>,
    recorded: Vec<String>,
    reused: usize,
    pruned: usize,
    failures: Vec<(usize, IndexFailure)>,
    skips: Vec<IndexSkip>,
}

/// Decides each walked file: a file read whole joins the seen set and is reused when its row
/// still describes it, else extracted; an oversize file is skipped and an unreadable one fails.
#[must_use]
pub fn plan(previous: &Previous, observed: Vec<Observed>, analysis: Key, force: bool) -> Plan {
    let reusable = !force && previous.analysis.as_deref() == Some(analysis.bytes().as_slice());
    let mut plan = Plan {
        recorded: previous.entries.keys().cloned().collect(),
        ..Plan::default()
    };
    let mut present = BTreeSet::new();
    for (position, file) in observed.into_iter().enumerate() {
        present.insert(file.path.clone());
        let entry = previous.entries.get(&file.path).filter(|_| reusable);
        plan.consider(position, file, entry);
    }
    plan.pruned = plan
        .recorded
        .iter()
        .filter(|path| !present.contains(*path))
        .count();
    plan
}

impl Plan {
    fn consider(&mut self, position: usize, file: Observed, entry: Option<&Entry>) {
        let Observed {
            path,
            language,
            measure,
            read,
        } = file;
        match read {
            Read::Oversize { size } => {
                let reason = io::Skipped::TooLarge { size }.to_string();
                self.skips.push(IndexSkip { path, reason });
            }
            Read::Unreadable { reason } => {
                let failure = IndexFailure { path, reason };
                self.failures.push((position, failure));
            }
            Read::Text(file) => {
                let digest = file.digest();
                let planned = Extract {
                    file,
                    measure,
                    digest,
                };
                let unchanged = entry.is_some_and(|entry| row_matches(entry, &planned, language));
                self.readable(position, planned, unchanged);
            }
        }
    }

    fn readable(&mut self, position: usize, planned: Extract, unchanged: bool) {
        self.seen.insert(planned.file.path.clone());
        if unchanged {
            self.reused += 1;
            return;
        }
        self.positions.push(position);
        self.extract.push(planned);
    }
}

// The row describes the text now: the same language, the same digest, and measured exactly when
// the file is measured now.
fn row_matches(entry: &Entry, planned: &Extract, language: Language) -> bool {
    entry.language == language.name()
        && entry.digest == planned.digest.bytes()
        && entry.measured == planned.measure
}

#[derive(Debug)]
pub(crate) struct NewRow {
    pub(crate) file: FileText,
    pub(crate) digest: Key,
    pub(crate) encoded: Encoded,
}

/// The plan joined to its parse results: the rows to write, the paths to delete, and the report
/// lists.
#[derive(Debug)]
pub struct Settled {
    pub(crate) rows: Vec<NewRow>,
    pub(crate) delete: Vec<String>,
    pub(crate) reused: usize,
    pub(crate) pruned: usize,
    pub(crate) failures: Vec<IndexFailure>,
    pub(crate) skips: Vec<IndexSkip>,
}

impl Settled {
    /// Returns the report of the run that wrote this plan to `database`, with the counts after
    /// the write and what the sweep deleted.
    #[must_use]
    pub fn report(
        self,
        database: PathBuf,
        generation: String,
        counts: RowCounts,
        evicted: EvictionReport,
    ) -> IndexReport {
        IndexReport {
            database,
            generation,
            indexed: self.rows.len(),
            reused: self.reused,
            pruned: self.pruned,
            files: counts.files,
            tags: counts.tags,
            imports: counts.imports,
            refs: counts.refs,
            failures: self.failures,
            skipped_files: self.skips,
            evicted,
        }
    }
}

/// Joins the encoded parse results, one for each file of `plan.extract` in its order, to the
/// plan. A file that does not parse fails, and its old row goes with the rows of files that were
/// not read whole.
///
/// # Panics
///
/// Panics when `parsed` does not hold one result for each planned file.
#[must_use]
pub fn settle(plan: Plan, parsed: Vec<Result<Encoded, Unparsable>>) -> Settled {
    assert_eq!(
        plan.extract.len(),
        parsed.len(),
        "one parse result for each planned file"
    );
    let Plan {
        extract,
        positions,
        mut seen,
        recorded,
        reused,
        pruned,
        mut failures,
        skips,
    } = plan;
    let mut rows = Vec::new();
    for ((planned, position), result) in extract.into_iter().zip(positions).zip(parsed) {
        match result {
            Ok(encoded) => rows.push(NewRow {
                file: planned.file,
                digest: planned.digest,
                encoded,
            }),
            Err(unparsable) => {
                seen.remove(&planned.file.path);
                let reason = unparsable.to_string();
                let failure = IndexFailure {
                    path: planned.file.path,
                    reason,
                };
                failures.push((position, failure));
            }
        }
    }
    failures.sort_by_key(|(position, _)| *position);
    rows.sort_by(|a, b| a.file.path.cmp(&b.file.path));
    let written: BTreeSet<&str> = rows.iter().map(|row| row.file.path.as_str()).collect();
    let delete = recorded
        .into_iter()
        .filter(|path| !seen.contains(path) || written.contains(path.as_str()))
        .collect();
    Settled {
        rows,
        delete,
        reused,
        pruned,
        failures: failures.into_iter().map(|(_, failure)| failure).collect(),
        skips,
    }
}
