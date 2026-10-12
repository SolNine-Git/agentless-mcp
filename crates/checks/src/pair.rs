//! Text pairs with each side parsed and measured once, and how a check reads the sides.

use std::collections::HashSet;
use std::sync::Arc;

use metrics::{FileMetrics, Unmeasurable};
use prompts::{Count, Template, Text, messages};
use syntax::{Language, Parsed};

use crate::error::{Error, SideError};
use crate::finding::{PairFinding, UNMEASURABLE_SOURCE, not_checked};

/// One file's whole text before and after a change, `None` for a side that does not exist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextPair {
    /// The repo-relative POSIX path.
    pub path: String,
    /// The text before the change.
    pub old: Option<String>,
    /// The text after the change.
    pub new: Option<String>,
}

/// One text pair with each side parsed and measured once, for every check of one run.
pub struct Pair {
    path: String,
    language: Option<Language>,
    old: Option<Side>,
    new: Option<Side>,
}

type Side = Result<Measured, SideError>;

pub(crate) struct Measured {
    pub(crate) parsed: Parsed,
    pub(crate) metrics: FileMetrics,
}

impl Pair {
    /// Parse and measure each side of `pair` that exists, when its path is tier-1 source.
    #[must_use]
    pub fn measure(pair: TextPair) -> Pair {
        let language = pair_language(&pair.path);
        let side = |text: Option<String>| Some(measured(language?, text?));
        Pair {
            old: side(pair.old),
            new: side(pair.new),
            language,
            path: pair.path,
        }
    }

    /// Return the repo-relative path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Return the language the checks read the pair as, or `None` for a pair they skip.
    #[must_use]
    pub fn language(&self) -> Option<Language> {
        self.language
    }

    pub(crate) fn has_new(&self) -> bool {
        self.new.is_some()
    }
}

/// Return the language the pair checks read `path` as, or `None` for a file they skip.
#[must_use]
pub fn pair_language(path: &str) -> Option<Language> {
    Language::from_path(path).filter(|language| language.spec().is_some())
}

/// Parse `text` as `language` and measure it: the in-memory block source of the clone index.
///
/// # Errors
///
/// Returns an error when the text does not parse into a usable tree, or does not measure.
pub fn measure_text(language: Language, text: &str) -> Result<FileMetrics, SideError> {
    measured(language, text).map(|side| side.metrics)
}

fn measured(language: Language, text: impl Into<Arc<str>>) -> Side {
    let parsed = syntax::parse(language, text.into()).map_err(SideError::Unparsable)?;
    let metrics = metrics::measure(&parsed).map_err(SideError::Unmeasurable)?;
    Ok(Measured { parsed, metrics })
}

static NOTHING: FileMetrics = FileMetrics {
    units: Vec::new(),
    blocks: Vec::new(),
    sloc_lines: Vec::new(),
};

// A side that does not exist measures as an empty text.
pub(crate) fn metrics(side: Option<&Measured>) -> &FileMetrics {
    side.map_or(&NOTHING, |measured| &measured.metrics)
}

pub(crate) struct Reasons {
    does_not_parse: &'static Template<(Text, Count)>,
    tokenizer: &'static Template<(Text, Text)>,
}

pub(crate) const COMPLEXITY_REASONS: Reasons = Reasons {
    does_not_parse: &messages::COMPLEXITY_DOES_NOT_PARSE,
    tokenizer: &messages::COMPLEXITY_TOKENIZER_REJECTS,
};

pub(crate) const CLONE_REASONS: Reasons = Reasons {
    does_not_parse: &messages::CLONE_DOES_NOT_PARSE,
    tokenizer: &messages::CLONE_TOKENIZER_REJECTS,
};

pub(crate) struct Gaps {
    check: &'static str,
    reasons: &'static Reasons,
    old: &'static Template<(Text, Text)>,
    new: &'static Template<(Text, Text)>,
}

pub(crate) const COMPLEXITY_GAPS: Gaps = Gaps {
    check: crate::COMPLEXITY_DELTA,
    reasons: &COMPLEXITY_REASONS,
    old: &messages::COMPLEXITY_OLD_NOT_MEASURED,
    new: &messages::COMPLEXITY_NEW_NOT_MEASURED,
};

pub(crate) const CONTRACT_GAPS: Gaps = Gaps {
    check: crate::CONTRACT_DRIFT,
    reasons: &COMPLEXITY_REASONS,
    old: &messages::CONTRACT_OLD_NOT_MEASURED,
    new: &messages::CONTRACT_NEW_NOT_MEASURED,
};

pub(crate) const CLONE_GAPS: Gaps = Gaps {
    check: crate::CLONE_DELTA,
    reasons: &CLONE_REASONS,
    old: &messages::CLONE_OLD_NOT_INDEXED,
    new: &messages::CLONE_NEW_NOT_INDEXED,
};

// A parse or tokenizer failure is a property of the text, so it becomes a row; any other
// failure is bad input or data, so it stops the run.
pub(crate) fn side_reason(
    path: &str,
    error: &SideError,
    reasons: &Reasons,
) -> Result<String, Error> {
    match error {
        SideError::Unmeasurable(Unmeasurable::DoesNotParse { line }) => {
            Ok(reasons.does_not_parse.render((path, *line as usize)))
        }
        SideError::Unmeasurable(Unmeasurable::Tokenizer(rejected)) => {
            Ok(reasons.tokenizer.render((path, &rejected.to_string())))
        }
        SideError::Recorded(reason) => Ok(reason.clone()),
        SideError::Unmeasurable(_) | SideError::Unparsable(_) => Err(Error::Side {
            path: path.to_owned(),
            source: error.clone(),
        }),
    }
}

pub(crate) enum Read<T> {
    Value(T),
    Row(PairFinding),
}

pub(crate) struct Sides<'a> {
    pub(crate) pair: &'a Pair,
    pub(crate) language: Language,
    pub(crate) old: Option<&'a Measured>,
    pub(crate) new: Option<&'a Measured>,
}

pub(crate) fn read_sides<'a>(
    pair: &'a Pair,
    language: Language,
    gaps: &Gaps,
) -> Result<Read<Sides<'a>>, Error> {
    // The old side is read first, and a failure there leaves the new side unread.
    let mut read = [None, None];
    let sides = [(pair.old.as_ref(), gaps.old), (pair.new.as_ref(), gaps.new)];
    for (slot, (side, template)) in read.iter_mut().zip(sides) {
        match read_side(pair, side, template, gaps)? {
            Read::Value(side) => *slot = side,
            Read::Row(row) => return Ok(Read::Row(row)),
        }
    }
    let [old, new] = read;
    Ok(Read::Value(Sides {
        pair,
        language,
        old,
        new,
    }))
}

fn read_side<'a>(
    pair: &Pair,
    side: Option<&'a Side>,
    template: &Template<(Text, Text)>,
    gaps: &Gaps,
) -> Result<Read<Option<&'a Measured>>, Error> {
    match side {
        None => Ok(Read::Value(None)),
        Some(Ok(measured)) => Ok(Read::Value(Some(measured))),
        Some(Err(error)) => {
            let reason = side_reason(&pair.path, error, gaps.reasons)?;
            let message = template.render((&pair.path, &reason));
            let row = not_checked(gaps.check, &pair.path, message, UNMEASURABLE_SOURCE, None);
            Ok(Read::Row(row))
        }
    }
}

pub(crate) fn each_pair<'a, T>(
    pairs: &[&'a Pair],
    mut read: impl FnMut(&'a Pair, Language) -> Result<Read<T>, Error>,
) -> Result<(Vec<T>, Vec<PairFinding>), Error> {
    refuse_repeats(pairs)?;
    let mut values = Vec::new();
    let mut rows = Vec::new();
    for pair in pairs {
        let Some(language) = pair.language else {
            continue;
        };
        match read(pair, language)? {
            Read::Value(value) => values.push(value),
            Read::Row(row) => rows.push(row),
        }
    }
    Ok((values, rows))
}

fn refuse_repeats(pairs: &[&Pair]) -> Result<(), Error> {
    let mut seen = HashSet::new();
    match pairs.iter().find(|pair| !seen.insert(pair.path.as_str())) {
        Some(pair) => Err(Error::RepeatedPath {
            path: pair.path.clone(),
        }),
        None => Ok(()),
    }
}
