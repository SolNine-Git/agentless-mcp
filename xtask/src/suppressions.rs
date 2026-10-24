use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;

use crate::sources::{self, Source};
use crate::tokens::{Item, Meta};
use crate::{Failure, repo, verdict, workspace};

const LISTED: &str = "crates/suppressions.toml";
// warn counts too: `#![warn(warnings)]` overrides `-D warnings`, so it lowers every lint.
const LINT_LEVELS: [&str; 3] = ["allow", "expect", "warn"];
const TOOLS: [&str; 2] = ["clippy", "rustfmt"];
// Each suppresses as a whole: rustfmt's legacy skip, as in #[cfg_attr(rustfmt, rustfmt_skip)], and
// #[ignore], which turns a test off.
const WHOLE: [&str; 2] = ["rustfmt_skip", "ignore"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Listed {
    allow: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    pub(crate) file: String,
    pub(crate) lint: String,
    #[serde(default = "one")]
    pub(crate) count: usize,
    reason: String,
}

struct Found {
    file: String,
    line: usize,
    lint: String,
}

/// Fail unless crates/suppressions.toml lists each lint-level and tool attribute, and each #[ignore], in crates/ and xtask/ exactly.
pub fn check(root: &Path) -> Result<(), Failure> {
    let listed = entries(root)?;
    let sources = sources::scan(root, &workspace::packages(root)?)?;
    let mut problems = sources.problems;
    let mut found = Vec::new();
    for source in &sources.files {
        let (suppressions, unreadable) = suppressions(source);
        found.extend(suppressions);
        problems.extend(unreadable);
    }
    problems.extend(compare(&listed, &found));
    verdict(problems)
}

pub(crate) fn entries(root: &Path) -> Result<Vec<Entry>, Failure> {
    let listed: Listed = repo::toml(&root.join(LISTED))?;
    Ok(listed.allow)
}

fn one() -> usize {
    1
}

fn suppressions(source: &Source) -> (Vec<Found>, Vec<String>) {
    let mut found = Vec::new();
    let mut problems = Vec::new();
    for meta in &source.scan.attributes {
        for lint in suppressed(meta) {
            match lint {
                Ok(lint) => found.push(Found {
                    file: source.file.clone(),
                    line: meta.line,
                    lint,
                }),
                Err(text) => problems.push(format!(
                    "{}:{}: the gates cannot read the lint `{text}` in #[{}]. Write the lint name literally",
                    source.file, meta.line, meta.path
                )),
            }
        }
    }
    (found, problems)
}

fn suppressed(meta: &Meta) -> Vec<Result<String, String>> {
    let tool = meta
        .path
        .split_once("::")
        .is_some_and(|(tool, _)| TOOLS.contains(&tool));
    if tool || WHOLE.contains(&meta.path.as_str()) {
        return vec![Ok(meta.path.clone())];
    }
    if !LINT_LEVELS.contains(&meta.path.as_str()) {
        return Vec::new();
    }
    meta.list
        .iter()
        .flatten()
        .filter_map(|item| match item {
            Item::Path(lint) => Some(Ok(lint.clone())),
            Item::Assignment => None,
            Item::Unreadable(text) => Some(Err(text.clone())),
        })
        .collect()
}

fn compare(listed: &[Entry], found: &[Found]) -> Vec<String> {
    let mut lines: BTreeMap<(&str, &str), Vec<usize>> = BTreeMap::new();
    for suppression in found {
        lines
            .entry((&suppression.file, &suppression.lint))
            .or_default()
            .push(suppression.line);
    }
    let mut problems = entry_problems(listed);
    for (&(file, lint), lines) in &lines {
        problems.extend(found_problems(listed, file, lint, lines));
    }
    problems.extend(
        listed
            .iter()
            .filter(|entry| !lines.contains_key(&(entry.file.as_str(), entry.lint.as_str())))
            .map(|entry| {
                format!(
                    "{LISTED}: {} in {} is stale: the file does not suppress it. Remove the entry",
                    entry.lint, entry.file
                )
            }),
    );
    problems
}

fn entry_problems(listed: &[Entry]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut problems = Vec::new();
    for entry in listed {
        let place = format!("{LISTED}: {} in {}", entry.lint, entry.file);
        if entry.reason.trim().is_empty() {
            problems.push(format!(
                "{place} has no reason. Write why the code needs the suppression"
            ));
        }
        if entry.count == 0 {
            problems.push(format!(
                "{place} has a count of 0. Remove the count, or set it to the number of suppressions in the file"
            ));
        }
        if !seen.insert((&entry.file, &entry.lint)) {
            problems.push(format!("{place} is listed twice. Merge the two entries"));
        }
    }
    problems
}

fn found_problems(listed: &[Entry], file: &str, lint: &str, lines: &[usize]) -> Vec<String> {
    match listed
        .iter()
        .find(|entry| entry.file == file && entry.lint == lint)
    {
        None => lines
            .iter()
            .map(|line| {
                format!(
                    "{file}:{line}: the {lint} suppression has no entry in {LISTED}. Remove it, or add an entry with its reason"
                )
            })
            .collect(),
        Some(entry) if entry.count != lines.len() => vec![format!(
            "{file}: {lint} is suppressed {} times, but {LISTED} lists {}. Make the two counts equal",
            lines.len(),
            entry.count
        )],
        Some(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Entry, Found, Listed, compare, suppressions};
    use crate::sources::Source;
    use crate::tokens;

    fn source(text: &str) -> Source {
        Source {
            file: "f.rs".to_owned(),
            scan: tokens::scan(text).unwrap(),
        }
    }

    fn lints(text: &str) -> Vec<(usize, String)> {
        let (found, problems) = suppressions(&source(text));
        assert!(problems.is_empty(), "{problems:?}");
        found
            .into_iter()
            .map(|found| (found.line, found.lint))
            .collect()
    }

    fn entry(lint: &str, count: usize, reason: &str) -> Entry {
        Entry {
            file: "f.rs".to_owned(),
            lint: lint.to_owned(),
            count,
            reason: reason.to_owned(),
        }
    }

    fn found(lint: &str, at: usize) -> Found {
        Found {
            file: "f.rs".to_owned(),
            line: at,
            lint: lint.to_owned(),
        }
    }

    fn expected(pairs: &[(usize, &str)]) -> Vec<(usize, String)> {
        pairs
            .iter()
            .map(|&(line, lint)| (line, lint.to_owned()))
            .collect()
    }

    #[test]
    fn every_attribute_form_is_found_with_its_line() {
        let text = "fn a() {}\n#[allow(dead_code)]\n#![expect(clippy::too_many_lines, reason = \"x\")]\n\
                    #[cfg_attr(test, allow(unused))]\n#[allow(\n    clippy::a,\n    clippy::b,\n)]\n\
                    #[expect/*c*/(dead_code)]\n";
        let pairs = [
            (2, "dead_code"),
            (3, "clippy::too_many_lines"),
            (4, "unused"),
            (5, "clippy::a"),
            (5, "clippy::b"),
            (9, "dead_code"),
        ];
        assert_eq!(lints(text), expected(&pairs));
    }

    #[test]
    fn a_method_named_expect_and_a_disallow_word_are_not_suppressions() {
        let text = "#[derive(Debug)]\nfn f() { x.expect(\"no\"); }\n#[doc = \"disallow(this)\"]\n";
        assert!(lints(text).is_empty());
    }

    #[test]
    fn rustfmt_and_clippy_tool_attributes_are_suppressions() {
        let text = "#[rustfmt::skip]\n#[rustfmt::skip::macros(m)]\n#[cfg_attr(rustfmt, rustfmt_skip)]\n\
                    #[clippy::msrv = \"1.0\"]\n";
        let pairs = [
            (1, "rustfmt::skip"),
            (2, "rustfmt::skip::macros"),
            (3, "rustfmt_skip"),
            (4, "clippy::msrv"),
        ];
        assert_eq!(lints(text), expected(&pairs));
    }

    #[test]
    fn an_ignored_test_is_a_suppression() {
        let text = "#[test]\n#[ignore]\nfn a() {}\n#[ignore = \"slow\"]\nfn b() {}\n\
                    #[cfg_attr(miri, ignore)]\nfn c() {}\n#[should_panic]\nfn d() {}\n";
        assert_eq!(
            lints(text),
            expected(&[(2, "ignore"), (4, "ignore"), (6, "ignore")])
        );
    }

    #[test]
    fn a_warn_attribute_is_a_suppression() {
        let text = "#![cfg_attr(all(), warn(warnings))]\n#[warn(clippy::dbg_macro)]\nfn f() {}\n";
        assert_eq!(
            lints(text),
            expected(&[(1, "warnings"), (2, "clippy::dbg_macro")])
        );
    }

    #[test]
    fn an_unreadable_lint_list_is_a_problem() {
        let text = "macro_rules! named { ($l:ident) => { #[expect($l)] fn f() {} }; }\n";
        let (found, problems) = suppressions(&source(text));
        assert!(found.is_empty());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn a_count_that_differs_from_its_entry_fails() {
        let twice = [found("dead_code", 2), found("dead_code", 5)];
        assert_eq!(
            compare(&[entry("dead_code", 1, "r")], &twice),
            [
                "f.rs: dead_code is suppressed 2 times, but crates/suppressions.toml lists 1. Make the two counts equal"
            ]
        );
        assert!(compare(&[entry("dead_code", 2, "r")], &twice).is_empty());
    }

    #[test]
    fn a_stale_entry_fails() {
        assert_eq!(
            compare(&[entry("dead_code", 1, "r")], &[]),
            [
                "crates/suppressions.toml: dead_code in f.rs is stale: the file does not suppress it. Remove the entry"
            ]
        );
    }

    #[test]
    fn an_unlisted_suppression_fails_on_each_line() {
        assert_eq!(
            compare(&[], &[found("unused", 4), found("unused", 7)]),
            [
                "f.rs:4: the unused suppression has no entry in crates/suppressions.toml. Remove it, or add an entry with its reason",
                "f.rs:7: the unused suppression has no entry in crates/suppressions.toml. Remove it, or add an entry with its reason",
            ]
        );
    }

    #[test]
    fn a_missing_count_means_one() {
        let listed: Listed =
            toml::from_str("allow = [{ file = \"f.rs\", lint = \"unused\", reason = \"r\" }]")
                .unwrap();
        assert_eq!(listed.allow[0].count, 1);
        assert!(compare(&listed.allow, &[found("unused", 3)]).is_empty());
    }

    #[test]
    fn review_a_rustdoc_lint_and_a_repeated_lint_are_counted() {
        let (found, problems) = suppressions(&source(
            "#[allow(rustdoc::broken_intra_doc_links, unused, unused)]\nfn f() {}\n",
        ));
        assert!(problems.is_empty());
        let lints: Vec<&str> = found.iter().map(|found| found.lint.as_str()).collect();
        assert_eq!(
            lints,
            ["rustdoc::broken_intra_doc_links", "unused", "unused"]
        );
        let entry = Entry {
            file: "f.rs".to_owned(),
            lint: "unused".to_owned(),
            count: 1,
            reason: "r".to_owned(),
        };
        let twice = [
            Found {
                file: "f.rs".to_owned(),
                line: 1,
                lint: "unused".to_owned(),
            },
            Found {
                file: "f.rs".to_owned(),
                line: 1,
                lint: "unused".to_owned(),
            },
        ];
        assert_eq!(compare(&[entry], &twice).len(), 1);
    }

    #[test]
    fn a_duplicate_entry_an_empty_reason_and_a_zero_count_fail() {
        let listed = [entry("unused", 1, " "), entry("unused", 0, "r")];
        assert_eq!(
            compare(&listed, &[found("unused", 3)]),
            [
                "crates/suppressions.toml: unused in f.rs has no reason. Write why the code needs the suppression",
                "crates/suppressions.toml: unused in f.rs has a count of 0. Remove the count, or set it to the number of suppressions in the file",
                "crates/suppressions.toml: unused in f.rs is listed twice. Merge the two entries",
            ]
        );
    }
}
