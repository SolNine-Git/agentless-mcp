use std::collections::BTreeSet;

use facts::{Facts, Ref, Role};
use graph::{FileFacts, NameIndex, Site, definitions_for, references_to};
use kernel::Stopped;
use syntax::Language;

use super::shared::shared_callers;
use super::{RefsRequest, RefsResult, dedupe, names_by_path, refs, round_robin};
use crate::IntArg;
use crate::clock::spent;
use crate::endpoints::scanned::{Scanned, TESTED};
use crate::endpoints::{BindingStatus, CoverageStatus};
use crate::scan::RepoFacts;

fn referenced(files: &[(&str, &str)], target: &str) -> RefsResult {
    referenced_at(files, target, 50)
}

fn referenced_at(files: &[(&str, &str)], target: &str, limit: usize) -> RefsResult {
    let scanned = Scanned::new(files);
    let request = RefsRequest {
        target: target.to_owned(),
        limit: IntArg::from_usize(limit),
        shared_callers: false,
    };
    let outcome = refs(&scanned.services, &scanned.reading(), &request);
    outcome.value.unwrap_or_else(|error| panic!("{error}"))
}

fn listed(result: &RefsResult) -> Vec<(&str, u32)> {
    let groups = result.groups.rows.iter();
    let rows =
        groups.flat_map(|group| (group.sites.iter()).map(|site| (group.path.as_str(), site.line)));
    rows.collect()
}

// Two classes that each define `as_sql`, and a module that calls one of them.
const CALLED: [(&str, &str); 2] = [
    (
        "a.py",
        "class Value:\n    def as_sql(self):\n        return 1\n\n\nclass Other:\n    def \
         as_sql(self):\n        return 2\n",
    ),
    (
        "b.py",
        "from a import Value\n\n\ndef use():\n    return Value().as_sql()\n",
    ),
];

#[test]
fn a_qualified_name_lists_the_references_of_its_one_definition() {
    let answer = referenced(&CALLED, "Value.as_sql");
    assert!(answer.target_resolved);
    assert_eq!(answer.definition.as_deref(), Some("py:a.py::Value.as_sql"));
    assert_eq!(listed(&answer), [("b.py", 5)]);
}

#[test]
fn a_declaration_is_never_a_reference() {
    let answer = referenced(&CALLED, "as_sql");
    assert!(answer.target_resolved);
    assert_eq!(answer.definition, None);
    assert_eq!(answer.definitions, 2);
    assert_eq!(listed(&answer), [("b.py", 5)]);
}

#[test]
fn a_bare_name_of_one_definition_names_that_definition() {
    let answer = referenced(&CALLED, "Value");
    assert!(answer.target_resolved);
    assert_eq!(answer.definition.as_deref(), Some("py:a.py::Value"));
    assert_eq!(answer.definitions, 1);
}

// A class of one name in two languages, its constructors, and a file that uses it.
const DECLARED: [(&str, &str); 3] = [
    (
        "Deco.java",
        "package x;\n\npublic class Deco {\n    public Deco() {\n    }\n\n    public Deco(int \
         level) {\n    }\n}\n",
    ),
    (
        "Deco.cs",
        "namespace X\n{\n    public class Deco\n    {\n    }\n}\n",
    ),
    (
        "Use.java",
        "package x;\n\nclass Use {\n    Object make() {\n        return new Deco();\n    }\n}\n",
    ),
];

#[test]
fn a_declaring_identifier_is_no_reference_in_any_language() {
    let answer = referenced(&DECLARED, "Deco");
    assert_eq!(answer.definitions, 4);
    assert_eq!(listed(&answer), [("Use.java", 5)]);
}

#[test]
fn a_declaring_line_that_spells_the_name_again_stays_a_reference_row() {
    let files = [
        (
            "Other.java",
            "class Other {\n    static final int CJ = 1;\n}\n",
        ),
        (
            "Holder.java",
            "class Holder {\n    static final int CJ = Other.CJ;\n}\n",
        ),
    ];
    let answer = referenced(&files, "CJ");
    assert_eq!(answer.definitions, 2);
    assert_eq!(listed(&answer), [("Holder.java", 2)]);
}

#[test]
fn a_data_key_id_has_no_fan_in() {
    let files = [
        ("data.json", "{\n  \"receipt_header\": \"x\"\n}\n"),
        (
            "use.py",
            "def receipt_header():\n    return 1\n\n\ndef caller():\n    return receipt_header()\n",
        ),
    ];
    let answer = referenced(&files, "json:data.json::receipt_header");
    assert_eq!(answer.data_key.as_deref(), Some("receipt_header"));
    assert!(answer.groups.rows.is_empty());
    assert_eq!(answer.groups.total, 0);
}

#[test]
fn a_data_file_id_that_names_no_key_gives_the_reason_and_lists_no_code_site() {
    let files = [
        ("data.json", "{\n  \"receipt_header\": \"x\"\n}\n"),
        (
            "use.py",
            "def caller():\n    return 1\n\n\ndef other():\n    return caller()\n",
        ),
    ];
    let answer = referenced(&files, "json:data.json::caller");
    assert_eq!(
        answer.unresolved.as_deref(),
        Some(
            "data.json keeps an outline of its keys, and caller is not in it: the outline stops \
             at depth 2, at list items and at 500 keys. Read the lines with read slice, or Grep \
             for the key."
        )
    );
    assert!(!answer.target_resolved);
    assert_eq!(answer.data_key, None);
    assert_eq!(answer.groups.total, 0);
}

// `helper` in a.py and e.ts, an alias of the a.py one in b.py, a module qualifier in c.py, and a
// TypeScript caller of the e.ts one in d.ts.
const ALIASED: [(&str, &str); 5] = [
    (
        "a.py",
        "def helper(value):\n    return value + 1\n\n\ndef other():\n    return helper(2)\n",
    ),
    (
        "b.py",
        "from a import helper as assist\n\n\ndef run():\n    return assist(1)\n\n\ndef \
         again():\n    assist(2)\n    return assist(3)\n",
    ),
    ("c.py", "import a\n\n\ndef go():\n    return a.helper(4)\n"),
    (
        "d.ts",
        "import { helper } from \"./e\";\n\nexport function useIt(): number {\n  return \
         helper(1);\n}\n",
    ),
    (
        "e.ts",
        "export function helper(x: number): number {\n  return x;\n}\n",
    ),
];

#[test]
fn a_path_qualified_name_binds_as_its_stable_id_does() {
    let answer = referenced(&CALLED, "a.py::Value.as_sql");
    assert_eq!(answer.binding.status, BindingStatus::Bound);
    assert_eq!(answer.binding.form, "path_name");
    assert_eq!(answer.definition.as_deref(), Some("py:a.py::Value.as_sql"));
    assert_eq!(listed(&answer), [("b.py", 5)]);
}

#[test]
fn a_line_binds_the_innermost_symbol_that_spans_it() {
    let answer = referenced(&CALLED, "a.py:3");
    assert_eq!(answer.binding.form, "path_line");
    assert_eq!(answer.definition.as_deref(), Some("py:a.py::Value.as_sql"));
    assert_eq!(listed(&answer), [("b.py", 5)]);
}

#[test]
fn a_stable_id_of_a_file_the_scan_did_not_read_lists_candidates_and_no_rows() {
    let answer = referenced(&CALLED, "py:c.py::Value.as_sql");
    assert_eq!(answer.binding.status, BindingStatus::NotFound);
    assert_eq!(
        answer.binding.message,
        "no symbol matches py:c.py::Value.as_sql: the scan read no file c.py. 2 definitions of \
         as_sql: py:a.py::Value.as_sql, py:a.py::Other.as_sql. Pass one as the target."
    );
    assert!(answer.groups.rows.is_empty());
    assert!(!answer.target_resolved);
    assert_eq!(answer.coverage.status, CoverageStatus::NotChecked);
}

#[test]
fn a_name_that_no_file_declares_says_how_many_files_spell_it() {
    let files = [
        ("a.py", "def run():\n    return ghost()\n"),
        ("b.py", "def other():\n    return ghost\n"),
    ];
    let answer = referenced(&files, "ghost");
    assert_eq!(answer.binding.status, BindingStatus::NotFound);
    assert_eq!(
        answer.binding.message,
        "no symbol matches ghost. 2 files spell ghost in code, and no file declares it. Grep \
         finds every spelling."
    );
    assert_eq!(answer.groups.total, 0);
}

#[test]
fn a_bare_path_names_a_file_and_binds_to_nothing() {
    let answer = referenced(&CALLED, "b.py");
    assert_eq!(answer.binding.form, "path");
    assert_eq!(
        answer.binding.message,
        "no symbol matches b.py: b.py is a file. Pass the stable id of one of its symbols"
    );
}

#[test]
fn the_bound_count_covers_every_site_before_the_limit_cuts_them() {
    let answer = referenced_at(&ALIASED, "py:a.py::helper", 1);
    let groups = &answer.groups;
    assert_eq!((groups.total, groups.bound, groups.shown()), (8, 6, 1));
}

#[test]
fn a_skipped_code_file_makes_the_check_partial_and_a_skipped_data_file_does_not() {
    let helper = "def helper(value):\n    return value + 1\n";
    let code = format!("{}helper(2)\n", "x = 1\n".repeat(200_000));
    let answer = referenced(&[("a.py", helper), ("big.py", &code)], "helper");
    let coverage = answer.coverage;
    assert_eq!(
        (coverage.status, coverage.skipped_code_files),
        (CoverageStatus::Partial, 1)
    );
    let data = format!("[{}1]\n", "1, ".repeat(400_000));
    let answer = referenced(&[("a.py", helper), ("big.json", &data)], "helper");
    let coverage = answer.coverage;
    assert_eq!(
        (coverage.status, coverage.skipped_code_files),
        (CoverageStatus::Complete, 0)
    );
    assert_eq!(answer.skipped.len(), 1);
}

fn file(path: &str, lines: &[u32]) -> FileFacts {
    let refs = lines.iter().map(|&line| Ref {
        name: format!("n{line}"),
        line,
        role: Role::Reference,
        qualifier: String::new(),
    });
    FileFacts {
        path: path.to_owned(),
        language: Language::Python,
        line_count: 100,
        facts: Facts {
            refs: refs.collect(),
            ..Facts::default()
        },
    }
}

fn sites(files: &[FileFacts]) -> Vec<Site<'_>> {
    let each = files.iter().flat_map(|file| {
        let refs = file.facts.refs.iter();
        refs.map(move |reference| Site { file, reference })
    });
    each.collect()
}

fn places(sites: &[Site<'_>]) -> Vec<(String, u32)> {
    let places = sites
        .iter()
        .map(|site| (site.path().to_owned(), site.reference.line));
    places.collect()
}

#[test]
fn one_site_per_line_in_file_then_line_order() {
    let files = [file("b.py", &[9, 3, 3]), file("a.py", &[5])];
    let spelled = sites(&files);
    let unique = dedupe(&spelled);
    let expected =
        [("a.py", 5), ("b.py", 3), ("b.py", 9)].map(|(path, line)| (path.to_owned(), line));
    assert_eq!(places(&unique), expected);
    let names = names_by_path(&spelled);
    assert_eq!(names["b.py"].len(), 2);
}

#[test]
fn the_limit_takes_one_site_per_file_in_turn() {
    let files = [
        file("a.py", &[1, 2, 3]),
        file("b.py", &[7]),
        file("c.py", &[4, 8]),
    ];
    let unique = dedupe(&sites(&files));
    let kept = round_robin(&unique, 5);
    let expected = [
        ("a.py", 1),
        ("b.py", 7),
        ("c.py", 4),
        ("a.py", 2),
        ("c.py", 8),
    ];
    let expected = expected.map(|(path, line)| (path.to_owned(), line));
    assert_eq!(places(&kept), expected);
    assert_eq!(round_robin(&unique, 100).len(), 6);
}

#[test]
fn the_shared_caller_ranking_stops_under_a_spent_deadline() {
    let scanned = Scanned::new(&TESTED);
    let loaded = RepoFacts::load(&scanned.services, &scanned.reading());
    let facts = loaded.value.unwrap_or_else(|error| panic!("{error}"));
    let index = NameIndex::build(&facts.files);
    let definitions = definitions_for(&index, "helper");
    let sites: Vec<Site<'_>> = (definitions.iter())
        .flat_map(|definition| references_to(&index, definition))
        .collect();
    let target_ids: BTreeSet<String> = (definitions.iter())
        .map(|definition| definition.stable_id().to_string())
        .collect();

    let ranked = shared_callers(
        &sites,
        &target_ids,
        &index,
        &BTreeSet::new(),
        Some(&spent()),
    );

    assert!(matches!(ranked, Err(Stopped::Spent { .. })), "{ranked:?}");
}
