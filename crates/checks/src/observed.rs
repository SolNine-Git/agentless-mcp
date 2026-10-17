//! The state a check observed in the new texts, beside its findings.

use kernel::Key;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

use crate::finding::{Match, PairFinding};
use crate::schema::{self, keyed};

/// A pair check's findings in their fixed order, and the state it observed in the new texts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Outcome {
    /// The findings, sorted by path, line, message, and evidence.
    pub findings: Vec<PairFinding>,
    /// The observed state.
    pub observed: Observed,
}

/// The state of the new texts; a field is `None` when its check measured no file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Observed {
    /// Every new unit with complexity above the threshold.
    pub units_over_threshold: Option<Vec<UnitState>>,
    /// Every new unit that a change added or altered.
    pub units_changed: Option<Vec<UnitChange>>,
    /// Every contract hit in the new texts.
    pub contract_keys: Option<Vec<HitState>>,
    /// Every new block that `clone_delta` could report.
    pub clone_instances: Option<Vec<CloneState>>,
}

impl Observed {
    /// Return this state with each field it does not hold taken from `other`.
    #[must_use]
    pub fn merged(self, other: Observed) -> Observed {
        Observed {
            units_over_threshold: self.units_over_threshold.or(other.units_over_threshold),
            units_changed: self.units_changed.or(other.units_changed),
            contract_keys: self.contract_keys.or(other.contract_keys),
            clone_instances: self.clone_instances.or(other.clone_instances),
        }
    }
}

// A list the state does not hold is left out of the object, not written as null.
impl Serialize for Observed {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let held = [
            self.units_over_threshold.is_some(),
            self.units_changed.is_some(),
            self.contract_keys.is_some(),
            self.clone_instances.is_some(),
        ];
        let fields = held.iter().filter(|&&is_held| is_held).count();
        let mut row = serializer.serialize_struct("Observed", fields)?;
        let over = self.units_over_threshold.as_ref();
        listed(&mut row, schema::UNITS_OVER_THRESHOLD, over)?;
        listed(&mut row, schema::UNITS_CHANGED, self.units_changed.as_ref())?;
        listed(&mut row, schema::CONTRACT_KEYS, self.contract_keys.as_ref())?;
        listed(
            &mut row,
            schema::CLONE_INSTANCES,
            self.clone_instances.as_ref(),
        )?;
        row.end()
    }
}

fn listed<R: SerializeStruct, T: Serialize>(
    row: &mut R,
    key: &'static str,
    list: Option<&Vec<T>>,
) -> Result<(), R::Error> {
    match list {
        Some(list) => row.serialize_field(key, list),
        None => row.skip_field(key),
    }
}

/// One unit of a new text with complexity above the threshold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitState {
    /// The repo-relative path.
    pub path: String,
    /// The qualified name.
    pub unit: String,
    /// The first line.
    pub line: u32,
    /// The complexity.
    pub cc: u32,
    /// The source line count.
    pub sloc: u32,
}

/// One unit a change added or altered, with no old side for a new unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitChange {
    /// The repo-relative path.
    pub path: String,
    /// The qualified name in the new text.
    pub unit: String,
    /// The first line in the new text.
    pub line: u32,
    /// How the unit was paired.
    pub via: Match,
    /// The old complexity.
    pub cc_old: Option<u32>,
    /// The new complexity.
    pub cc_new: u32,
    /// The old source line count.
    pub sloc_old: Option<u32>,
    /// The new source line count.
    pub sloc_new: u32,
}

/// One contract hit in a new text, named by the unit that holds it in that text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HitState {
    /// The repo-relative path.
    pub path: String,
    /// The rule id.
    pub rule: String,
    /// The owning unit's qualified name in the new text, or `<module>`.
    pub unit: String,
    /// The hit's normalized text.
    pub capture: String,
}

/// One block of a new text that `clone_delta` could report: big enough, and with a copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloneState {
    /// The repo-relative path.
    pub path: String,
    /// The Type-2 key.
    pub key: Key,
    /// The first line.
    pub line: u32,
    /// The last line.
    pub end_line: u32,
}

keyed! {
    UnitState { PATH: path, UNIT: unit, LINE: line, CC: cc, SLOC: sloc }
    UnitChange {
        PATH: path,
        UNIT: unit,
        LINE: line,
        MATCH: via,
        CC_OLD: cc_old,
        CC_NEW: cc_new,
        SLOC_OLD: sloc_old,
        SLOC_NEW: sloc_new,
    }
    HitState { PATH: path, RULE: rule, UNIT: unit, CAPTURE: capture }
    CloneState { PATH: path, KEY: key, LINE: line, END_LINE: end_line }
}
