//! Text pairs with each side parsed and measured once, and how a check reads the sides.

use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use metrics::{Block, CLONE_REASONS, COMPLEXITY_REASONS, Measurable, Reasons, Unit};
use prompts::{Template, Text, messages};
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

/// One side of a pair: its text parsed and measured, or why it did not measure.
pub struct Side {
    text: Arc<str>,
    outcome: Result<Measured, SideError>,
}

/// One text of a pair that the checks measure, with the language they read it as.
#[derive(Clone, Copy, Debug)]
pub struct SideText<'a> {
    language: Language,
    text: &'a str,
}

impl SideText<'_> {
    /// Parse and measure the text.
    #[must_use]
    pub fn measure(&self) -> Side {
        let text: Arc<str> = Arc::from(self.text);
        Side {
            outcome: measured(self.language, Arc::clone(&text)),
            text,
        }
    }
}

impl Side {
    /// Find the clone blocks of a side that measured, so that each later read finds them.
    pub fn find_blocks(&self) {
        if let Ok(measured) = &self.outcome {
            measured.blocks();
        }
    }
}

pub(crate) struct Measured {
    measurable: Measurable,
    pub(crate) units: Vec<Unit>,
    blocks: OnceLock<Vec<Block>>,
}

impl Measured {
    pub(crate) fn parsed(&self) -> &Parsed {
        self.measurable.parsed()
    }

    // Only clone_delta reads blocks, so a side finds them on the first read.
    pub(crate) fn blocks(&self) -> &[Block] {
        self.blocks.get_or_init(|| self.measurable.blocks())
    }
}

impl Pair {
    /// Measure each side of `pairs` that exists, with `map` turning the texts into sides in order.
    ///
    /// # Panics
    ///
    /// Panics when `map` returns a different number of sides than it was given texts.
    #[must_use]
    pub fn measure_all(
        pairs: Vec<TextPair>,
        map: impl FnOnce(&[SideText<'_>]) -> Vec<Side>,
    ) -> Vec<Pair> {
        let languages: Vec<Option<Language>> =
            pairs.iter().map(|pair| pair_language(&pair.path)).collect();
        let sides = {
            let texts: Vec<SideText<'_>> = pairs
                .iter()
                .zip(&languages)
                .flat_map(|(pair, language)| side_texts(pair, *language))
                .collect();
            let sides = map(&texts);
            assert_eq!(
                sides.len(),
                texts.len(),
                "the map returns one side per text"
            );
            sides
        };
        let mut sides = sides.into_iter();
        pairs
            .into_iter()
            .zip(languages)
            .map(|(pair, language)| assembled(pair, language, &mut sides))
            .collect()
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

    /// Return each side that exists, the old side first.
    pub fn sides(&self) -> impl Iterator<Item = &Side> {
        [&self.old, &self.new].into_iter().flatten()
    }

    /// Return the clone blocks of the side whose text is `text`, or `None` when no side has it.
    #[must_use]
    pub fn measured_blocks(&self, text: &str) -> Option<Result<Vec<Block>, SideError>> {
        let side = self.sides().find(|side| *side.text == *text)?;
        Some(match &side.outcome {
            Ok(measured) => Ok(measured.blocks().to_vec()),
            Err(error) => Err(error.clone()),
        })
    }

    pub(crate) fn has_new(&self) -> bool {
        self.new.is_some()
    }
}

// The old text, then the new text, of a pair whose path is tier-1 source.
fn side_texts(pair: &TextPair, language: Option<Language>) -> impl Iterator<Item = SideText<'_>> {
    [&pair.old, &pair.new].into_iter().filter_map(move |text| {
        Some(SideText {
            language: language?,
            text: text.as_deref()?,
        })
    })
}

fn assembled(
    pair: TextPair,
    language: Option<Language>,
    sides: &mut impl Iterator<Item = Side>,
) -> Pair {
    let mut next = |text: Option<String>| text.and(language).and_then(|_| sides.next());
    Pair {
        old: next(pair.old),
        new: next(pair.new),
        language,
        path: pair.path,
    }
}

/// Return the language the pair checks read `path` as, or `None` for a file they skip.
#[must_use]
pub fn pair_language(path: &str) -> Option<Language> {
    Language::from_path(path).filter(|language| language.spec().is_some())
}

/// Parse `text` as `language` and find its clone blocks: the in-memory block source of the index.
///
/// # Errors
///
/// Returns an error when the text does not parse into a usable tree, or does not measure.
pub fn measure_blocks(language: Language, text: &str) -> Result<Vec<Block>, SideError> {
    measurable(language, text.into()).map(|measurable| measurable.blocks())
}

fn measured(language: Language, text: Arc<str>) -> Result<Measured, SideError> {
    let measurable = measurable(language, text)?;
    Ok(Measured {
        units: measurable.units(),
        measurable,
        blocks: OnceLock::new(),
    })
}

fn measurable(language: Language, text: Arc<str>) -> Result<Measurable, SideError> {
    let parsed = syntax::parse(language, text).map_err(SideError::Unparsable)?;
    Measurable::new(parsed).map_err(SideError::Unmeasurable)
}

// A side that does not exist measures as an empty text.
pub(crate) fn units(side: Option<&Measured>) -> &[Unit] {
    side.map_or(&[], |measured| &measured.units)
}

pub(crate) fn blocks(side: Option<&Measured>) -> &[Block] {
    side.map_or(&[], Measured::blocks)
}

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
    let worded = match error {
        SideError::Unmeasurable(unmeasurable) => unmeasurable.reason(path, reasons),
        SideError::Recorded(reason) => Some(reason.clone()),
        SideError::Unparsable(_) => None,
    };
    worded.ok_or_else(|| Error::Side {
        path: path.to_owned(),
        source: error.clone(),
    })
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
    match side.map(|side| &side.outcome) {
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
