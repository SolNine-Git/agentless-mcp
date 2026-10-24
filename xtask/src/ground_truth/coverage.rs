use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::cargo_clippy::Diagnostic;
use super::lints::WARNINGS;
use crate::repo;
use crate::suppressions::Entry;

/// The ledger entries and the lint groups that decide whether a force-warned diagnostic is listed.
pub struct Ledger<'a> {
    pub entries: &'a [Entry],
    pub groups: &'a BTreeMap<String, Vec<String>>,
}

/// Return a problem for each force-warned diagnostic that no ledger entry of its target covers, once per place and lint.
pub fn problems(
    root: &Path,
    diagnostics: &[Diagnostic],
    files: &BTreeMap<PathBuf, BTreeSet<String>>,
    ledger: &Ledger,
) -> Vec<String> {
    let unlisted: BTreeSet<String> = diagnostics
        .iter()
        .filter(|diagnostic| {
            !files
                .get(&diagnostic.target)
                .is_some_and(|target_files| ledger.covers(target_files, &diagnostic.lint))
        })
        .map(|diagnostic| {
            format!(
                "{}: clippy --force-warn reports {} ({}) in the target {}, and crates/suppressions.toml has no entry for a file of that target that names the lint, a group that holds it, or warnings. Fix the code and remove any suppression of the lint, or write the suppression as a literal attribute and add its entry with the reason",
                diagnostic.place,
                diagnostic.lint,
                diagnostic.message,
                repo::relative(&diagnostic.target, root)
            )
        })
        .collect();
    unlisted.into_iter().collect()
}

impl Ledger<'_> {
    // An attribute in one file can suppress a lint in another file of its target: an inner attribute at the
    // crate root, an attribute on `mod`, or dead_code that keeps the callees of an allowed item alive.
    fn covers(&self, target_files: &BTreeSet<String>, lint: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| target_files.contains(&entry.file) && self.names(&entry.lint, lint))
    }

    fn names(&self, listed: &str, lint: &str) -> bool {
        listed == lint
            || listed == WARNINGS
            || self
                .groups
                .get(listed)
                .is_some_and(|members| members.iter().any(|member| member == lint))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    use super::{Ledger, problems};
    use crate::ground_truth::cargo_clippy::Diagnostic;
    use crate::suppressions::Entry;

    const KERNEL: &str = "/r/crates/kernel/src/lib.rs";
    const XTASK: &str = "/r/xtask/src/main.rs";

    fn diagnostic(target: &str, lint: &str, place: &str) -> Diagnostic {
        Diagnostic {
            target: PathBuf::from(target),
            lint: lint.to_owned(),
            place: place.to_owned(),
            message: "m".to_owned(),
        }
    }

    fn entries(pairs: &[(&str, &str)]) -> Vec<Entry> {
        pairs
            .iter()
            .map(|(file, lint)| {
                toml::from_str(&format!(
                    "file = \"{file}\"\nlint = \"{lint}\"\nreason = \"r\"\n"
                ))
                .unwrap()
            })
            .collect()
    }

    fn unlisted(diagnostics: &[Diagnostic], pairs: &[(&str, &str)]) -> Vec<String> {
        let files = BTreeMap::from([
            (
                PathBuf::from(KERNEL),
                BTreeSet::from([
                    "crates/kernel/src/lib.rs".to_owned(),
                    "crates/kernel/src/id.rs".to_owned(),
                ]),
            ),
            (
                PathBuf::from(XTASK),
                BTreeSet::from(["xtask/src/main.rs".to_owned()]),
            ),
        ]);
        let groups = BTreeMap::from([(
            "clippy::pedantic".to_owned(),
            vec!["clippy::must_use_candidate".to_owned()],
        )]);
        let entries = entries(pairs);
        let ledger = Ledger {
            entries: &entries,
            groups: &groups,
        };
        problems(Path::new("/r"), diagnostics, &files, &ledger)
            .into_iter()
            .map(|problem| {
                let head = problem.split(" (").next().unwrap();
                head.replace(": clippy --force-warn reports ", ": ")
            })
            .collect()
    }

    #[test]
    fn an_entry_for_a_file_of_the_target_covers_its_lint_its_group_members_or_every_lint() {
        let found = [
            diagnostic(KERNEL, "dead_code", "crates/kernel/src/id.rs:3"),
            diagnostic(
                KERNEL,
                "clippy::must_use_candidate",
                "crates/kernel/src/id.rs:9",
            ),
        ];
        assert!(
            unlisted(
                &found,
                &[
                    ("crates/kernel/src/lib.rs", "dead_code"),
                    ("crates/kernel/src/id.rs", "clippy::pedantic")
                ]
            )
            .is_empty()
        );
        assert!(unlisted(&found, &[("crates/kernel/src/lib.rs", "warnings")]).is_empty());
        assert_eq!(
            unlisted(&found, &[("crates/kernel/src/lib.rs", "clippy::all")]),
            [
                "crates/kernel/src/id.rs:3: dead_code",
                "crates/kernel/src/id.rs:9: clippy::must_use_candidate",
            ]
        );
    }

    #[test]
    fn an_entry_for_a_file_of_another_target_covers_nothing() {
        let found = [diagnostic(XTASK, "dead_code", "xtask/src/main.rs:4")];
        assert_eq!(
            unlisted(&found, &[("crates/kernel/src/lib.rs", "dead_code")]),
            ["xtask/src/main.rs:4: dead_code"]
        );
        let unbuilt = [diagnostic("/r/xtask/src/other.rs", "dead_code", "x:1")];
        assert_eq!(
            unlisted(&unbuilt, &[("xtask/src/main.rs", "dead_code")]),
            ["x:1: dead_code"]
        );
    }

    #[test]
    fn one_place_reported_by_two_units_is_one_problem() {
        let twice = [
            diagnostic(KERNEL, "dead_code", "crates/kernel/src/id.rs:3"),
            diagnostic(KERNEL, "dead_code", "crates/kernel/src/id.rs:3"),
        ];
        assert_eq!(unlisted(&twice, &[]).len(), 1);
    }
}
