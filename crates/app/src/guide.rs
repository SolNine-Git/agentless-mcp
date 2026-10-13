//! The agent usage guide that ships in the binary, and its sections.

use text::CodePointString;

use crate::generated::guide::{GUIDE, SECTIONS};

/// Returns the whole agent guide.
#[must_use]
pub fn guide_text() -> &'static str {
    GUIDE
}

/// Returns every section name, in document order.
pub fn section_names() -> impl Iterator<Item = &'static str> {
    SECTIONS.iter().map(|&(name, _, _)| name)
}

/// Returns the text of the section `name`, or `None` when no section has that name.
#[must_use]
pub fn section_text(name: &str) -> Option<&'static str> {
    let &(_, start, end) = SECTIONS
        .iter()
        .find(|(candidate, _, _)| *candidate == name)?;
    GUIDE.get(start..end)
}

/// Returns the refusal for the unknown section `name`. It quotes the name and lists every
/// section name.
#[must_use]
pub fn unknown_section(name: &CodePointString) -> String {
    let names: Vec<&str> = section_names().collect();
    format!(
        "no guide section named {}. Sections are: {}",
        name.quoted(),
        names.join(", ")
    )
}

#[cfg(test)]
mod tests;
