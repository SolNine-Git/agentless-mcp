//! The lowercase form of a text, from the Unicode 15.1 case mappings.

use std::ops::RangeInclusive;

/// Returns `text` lowercased by the Unicode 15.1 case mappings, with a capital sigma that ends a
/// word written as a final sigma.
#[must_use]
pub fn lower(text: &str) -> String {
    let mut lowered = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(newer_capital) {
        let (run, tail) = rest.split_at(at);
        lowered.push_str(&run.to_lowercase());
        let mut chars = tail.chars();
        lowered.extend(chars.next());
        rest = chars.as_str();
    }
    lowered.push_str(&rest.to_lowercase());
    lowered
}

// The capitals that Unicode 16 and 17 added. In Unicode 15.1 they are unassigned: they keep their
// case, and a sigma beside one reads it as neither cased nor case-ignorable.
const NEWER_CAPITALS: [RangeInclusive<u32>; 9] = [
    0x1c89..=0x1c89,
    0xa7cb..=0xa7cc,
    0xa7ce..=0xa7ce,
    0xa7d2..=0xa7d2,
    0xa7d4..=0xa7d4,
    0xa7da..=0xa7da,
    0xa7dc..=0xa7dc,
    0x10d50..=0x10d65,
    0x16ea0..=0x16eb8,
];

fn newer_capital(c: char) -> bool {
    let code = u32::from(c);
    NEWER_CAPITALS.iter().any(|range| range.contains(&code))
}

#[cfg(test)]
mod tests;
