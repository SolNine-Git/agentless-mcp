//! The grammar warmup: each grammar's tier, ABI and probe parse. Every grammar is linked into the
//! binary, so nothing downloads and every known grammar is warm.

use syntax::{Language, Tier};

use crate::LanguageCapability;
use crate::capabilities::language;

/// A warmup request: the language names to probe, or every language when it names none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WarmupRequest {
    /// The names, as the caller wrote them.
    pub languages: Vec<String>,
}

/// The probe of each language, in request order, or of every language in name order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WarmupReport {
    /// The release of the grammar sources.
    pub pack_version: &'static str,
    /// One row per name.
    pub rows: Vec<WarmupRow>,
    /// True when the caller named the languages.
    pub requested: bool,
}

/// One language of a warmup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WarmupRow {
    /// A linked grammar and its probe.
    Linked(LanguageCapability),
    /// A name that no linked grammar has.
    Unknown(String),
}

impl WarmupRow {
    /// Returns true when the grammar is warm and its probe parsed cleanly.
    #[must_use]
    pub fn usable(&self) -> bool {
        matches!(self, WarmupRow::Linked(row) if row.warmed && row.probe_ok)
    }

    /// Returns the support tier: an unknown name counts as tier 2.
    #[must_use]
    pub fn tier(&self) -> u8 {
        match self {
            WarmupRow::Linked(row) => row.tier,
            WarmupRow::Unknown(_) => Tier::Two.number(),
        }
    }
}

impl WarmupReport {
    /// Returns the rows that fail the run: every unusable row when the caller named the
    /// languages, else the unusable tier-1 rows.
    #[must_use]
    pub fn fatal(&self) -> usize {
        let tier_one = Tier::One.number();
        let fatal = |row: &&WarmupRow| self.requested || row.tier() == tier_one;
        self.degraded().filter(fatal).count()
    }

    /// Returns the unusable rows.
    pub fn degraded(&self) -> impl Iterator<Item = &WarmupRow> {
        self.rows.iter().filter(|row| !row.usable())
    }
}

/// Probes the grammars that `request` names, or every grammar.
#[must_use]
pub fn warmup(request: &WarmupRequest) -> WarmupReport {
    let requested = !request.languages.is_empty();
    let rows = if requested {
        let rows = request
            .languages
            .iter()
            .map(|name| match Language::from_name(name) {
                Some(known) => WarmupRow::Linked(language(known)),
                None => WarmupRow::Unknown(name.clone()),
            });
        rows.collect()
    } else {
        Language::ALL
            .map(|known| WarmupRow::Linked(language(known)))
            .to_vec()
    };
    WarmupReport {
        pack_version: syntax::PACK_VERSION,
        rows,
        requested,
    }
}

#[cfg(test)]
mod tests;
