use std::path::Path;

use serde::Deserialize;

use crate::{Failure, repo, verdict};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Listed {
    allow: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    file: String,
    lint: String,
    reason: String,
}

const KEYWORDS: [&str; 2] = ["allow(", "expect("];

/// Fail on any allow or expect attribute in `crates/` that `crates/suppressions.toml` does not list.
pub fn check(root: &Path) -> Result<(), Failure> {
    let listed: Listed = repo::toml(&root.join("crates/suppressions.toml"))?;
    let mut problems: Vec<String> = listed
        .allow
        .iter()
        .filter(|entry| entry.reason.trim().is_empty())
        .map(|entry| {
            format!(
                "crates/suppressions.toml: {} in {} has no reason",
                entry.lint, entry.file
            )
        })
        .collect();
    for path in repo::rust_files(&root.join("crates"))? {
        let file = repo::relative(&path, root);
        for (line, lint) in suppressed_lints(&repo::read(&path)?) {
            if !listed
                .allow
                .iter()
                .any(|entry| entry.file == file && entry.lint == lint)
            {
                problems.push(format!(
                    "{file}:{line}: the {lint} suppression has no entry in crates/suppressions.toml"
                ));
            }
        }
    }
    verdict(problems)
}

fn suppressed_lints(text: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for start in attribute_starts(text) {
        let attribute = enclosed(&text[start..], '[', ']');
        let line = text[..start].matches('\n').count() + 1;
        found.extend(lint_names(attribute).into_iter().map(|lint| (line, lint)));
    }
    found
}

fn attribute_starts(text: &str) -> Vec<usize> {
    let outer = text.match_indices("#[").map(|(index, _)| index);
    let inner = text.match_indices("#![").map(|(index, _)| index);
    let mut starts: Vec<usize> = outer.chain(inner).collect();
    starts.sort_unstable();
    starts
}

fn enclosed(text: &str, open: char, close: char) -> &str {
    let mut depth = 0_usize;
    for (index, character) in text.char_indices() {
        if character == open {
            depth += 1;
        } else if character == close {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return &text[..=index];
            }
        }
    }
    text
}

fn lint_names(attribute: &str) -> Vec<String> {
    let mut names = Vec::new();
    for keyword in KEYWORDS {
        for (index, _) in attribute.match_indices(keyword) {
            if starts_a_word(attribute, index) {
                let arguments = enclosed(&attribute[index..], '(', ')');
                names.extend(arguments_of(arguments));
            }
        }
    }
    names
}

fn starts_a_word(text: &str, index: usize) -> bool {
    text[..index]
        .chars()
        .next_back()
        .is_none_or(|before| !(before.is_alphanumeric() || before == '_'))
}

fn arguments_of(call: &str) -> Vec<String> {
    let inside = call
        .split_once('(')
        .map_or("", |(_, rest)| rest.strip_suffix(')').unwrap_or(rest));
    inside
        .split(',')
        .map(str::trim)
        .filter(|argument| !argument.is_empty() && !argument.contains('='))
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::suppressed_lints;

    #[test]
    fn every_attribute_form_is_found_with_its_line() {
        let text = "fn a() {}\n#[allow(dead_code)]\n#![expect(clippy::too_many_lines, reason = \"x\")]\n\
                    #[cfg_attr(test, allow(unused))]\n#[allow(\n    clippy::a,\n    clippy::b,\n)]\n";
        let found = suppressed_lints(text);
        let expected = [
            (2, "dead_code"),
            (3, "clippy::too_many_lines"),
            (4, "unused"),
            (5, "clippy::a"),
            (5, "clippy::b"),
        ];
        let expected: Vec<(usize, String)> = expected
            .iter()
            .map(|&(line, lint)| (line, lint.to_owned()))
            .collect();
        assert_eq!(found, expected);
    }

    #[test]
    fn a_method_named_expect_and_a_disallow_word_are_not_suppressions() {
        let text = "#[derive(Debug)]\nfn f() { x.expect(\"no\"); }\n#[doc = \"disallow(this)\"]\n";
        assert!(suppressed_lints(text).is_empty());
    }
}
