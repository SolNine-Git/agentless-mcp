use facts::SymbolKind;
use store::FileText;
use syntax::Language;

use super::{Usage, UsageFile, UsageOrder, UsageQuery, spelled, spellings, usage, words};
use crate::endpoints::scanned::Scanned;
use crate::scan::{Spelling, SymbolFacts};
use crate::{FindRequest, FindResult, IntArg, find};

fn usage_of(files: &[(&str, &str)], query: &str) -> Option<Usage> {
    let scanned = Scanned::new(files);
    let queries: Vec<UsageQuery> = UsageQuery::of(query).into_iter().collect();
    let words = words(&queries);
    let spell = |text: &FileText| spellings(text, &words);
    let loaded = SymbolFacts::load(&scanned.services, &scanned.reading(), &spell);
    let scan = loaded.value.unwrap_or_else(|error| panic!("{error}"));
    usage(&scan, &words, queries.first()?)
}

fn rows(usage: &Usage) -> Vec<(&str, u32, u32)> {
    let files = usage.files.rows().iter();
    files
        .map(|file| (file.path.as_str(), file.line, file.lines))
        .collect()
}

#[test]
fn a_name_is_spelled_only_where_no_word_character_touches_it() {
    let touched = "foobar foo_bar $foo foo$ \u{e9}foo foo\u{e9} foo1\n";
    assert_eq!(spelled(touched, "foo"), None);
    let alone = Some(Spelling { line: 1, lines: 1 });
    assert_eq!(spelled("x.foo(1)", "foo"), alone);
    assert_eq!(spelled("\u{2014}foo\u{2014}", "foo"), alone);
    assert_eq!(spelled("foo", "foo"), alone);
    assert_eq!(spelled("$scope = 1", "$scope"), alone);
}

#[test]
fn a_spelling_is_the_first_line_and_the_count_of_lines_that_spell_the_name() {
    let text = "a\nfoo(foo)\nbar\n  foo\nfoobar\n";
    assert_eq!(spelled(text, "foo"), Some(Spelling { line: 2, lines: 2 }));
}

#[test]
fn only_a_name_or_names_joined_by_a_dot_or_two_colons_have_a_usage_query() {
    let of = |query| UsageQuery::of(query).map(|found| (found.name, found.owner));
    assert_eq!(of("where"), Some(("where", None)));
    assert_eq!(of("Square.area"), Some(("area", Some("Square"))));
    assert_eq!(of("a::b::c"), Some(("c", Some("b"))));
    assert_eq!(of("$scope.x"), Some(("x", Some("$scope"))));
    assert_eq!(of("\u{3c9}mega"), Some(("\u{3c9}mega", None)));
    for query in ["a-b", "a(", "", "a.", ".a", "a b", "a:::b", "@a", "a<b"] {
        assert_eq!(of(query), None, "{query:?}");
    }
}

#[test]
fn the_words_hold_each_name_and_owner_once_in_query_order() {
    let queries: Vec<UsageQuery> = ["Square.area", "area", "Square"]
        .iter()
        .filter_map(|query| UsageQuery::of(query))
        .collect();
    assert_eq!(words(&queries), ["area", "Square"]);
}

#[test]
fn a_data_file_spells_no_word() {
    let text = |path: &str, language| FileText::decode(path.to_owned(), language, b"area: 1\n");
    let words = ["area", "zz"];
    assert_eq!(spellings(&text("a.yaml", Language::Yaml), &words), []);
    assert_eq!(
        spellings(&text("a.py", Language::Python), &words),
        [Some(Spelling { line: 1, lines: 1 }), None]
    );
}

#[test]
fn a_file_that_declares_the_name_in_its_case_and_a_data_file_are_not_usage_files() {
    let files = [
        ("decl.py", "def area():\n    return area\n"),
        (
            "other_case.py",
            "class Area:\n    pass\n\nprint(Area().area)\n",
        ),
        ("use.py", "from decl import area\n\nprint(area())\n"),
        ("conf.json", "{\"area\": 1}\n"),
    ];
    let found = usage_of(&files, "area").expect("two files spell area");
    assert_eq!(found.order, UsageOrder::NearestDeclaration);
    assert_eq!(rows(&found), [("use.py", 1, 2), ("other_case.py", 4, 1)]);
}

#[test]
fn a_qualified_query_keeps_the_files_that_also_spell_the_owner() {
    let files = [
        (
            "shapes.py",
            "class Square:\n    def area(self):\n        return 1\n",
        ),
        ("a.py", "from shapes import Square\n\nSquare().area()\n"),
        ("b.py", "def total(shape):\n    return shape.area()\n"),
    ];
    let found = usage_of(&files, "Square.area").expect("a.py spells both");
    assert_eq!(rows(&found), [("a.py", 3, 1)]);
    assert_eq!(
        (found.name.as_str(), found.owner.as_deref()),
        ("area", Some("Square"))
    );
}

#[test]
fn the_nearest_files_to_a_declaration_come_first_then_more_lines_then_the_path() {
    let files = [
        ("pkg/core/decl.py", "def area():\n    return 1\n"),
        ("pkg/core/near.py", "x = area\n"),
        ("pkg/z.py", "area\narea\narea\n"),
        ("pkg/b.py", "area\narea\narea\n"),
        ("pkg/core/sub/deep.py", "area\narea\n"),
        ("other/far.py", "area\narea\narea\narea\n"),
    ];
    let found = usage_of(&files, "area").expect("five files spell area");
    let paths: Vec<&str> = rows(&found).iter().map(|row| row.0).collect();
    assert_eq!(
        paths,
        [
            "pkg/core/near.py",
            "pkg/b.py",
            "pkg/z.py",
            "pkg/core/sub/deep.py",
            "other/far.py"
        ]
    );
}

#[test]
fn with_no_declaration_the_files_with_more_lines_come_first() {
    let files = [
        ("a.py", "# area\n"),
        ("b.py", "# area\n# area\n"),
        ("c.py", "# area\n"),
    ];
    let found = usage_of(&files, "area").expect("three files spell area");
    assert_eq!(found.order, UsageOrder::MostLines);
    assert_eq!(
        rows(&found),
        [("b.py", 1, 2), ("a.py", 1, 1), ("c.py", 1, 1)]
    );
}

#[test]
fn the_cap_keeps_ten_files_and_counts_all_of_them() {
    let names: Vec<String> = (0..12).map(|at| format!("m{at:02}.py")).collect();
    let files: Vec<(&str, &str)> = names
        .iter()
        .map(|name| (name.as_str(), "x = area\n"))
        .collect();
    let found = usage_of(&files, "area").expect("twelve files spell area");
    assert_eq!(
        (
            found.files.rows().len(),
            found.files.total(),
            found.files.omitted()
        ),
        (10, 12, 2)
    );
    assert_eq!(
        found.files.rows().last(),
        Some(&UsageFile {
            path: "m09.py".to_owned(),
            line: 1,
            lines: 1
        })
    );
}

#[test]
fn a_query_that_is_not_a_name_or_that_no_code_file_spells_has_no_usage() {
    let files = [("a.py", "x = 1  # a-b area\n"), ("d.json", "{\"zz\": 1}\n")];
    assert_eq!(usage_of(&files, "a-b"), None);
    assert_eq!(usage_of(&files, ""), None);
    assert_eq!(usage_of(&files, "zz"), None);
}

#[test]
fn a_kind_filter_and_a_declaration_miss_keep_the_usage_files() {
    let scanned = Scanned::new(&[
        ("pkg/mod.py", "def alpha():\n    return 1\n"),
        (
            "main.py",
            "from pkg.mod import alpha\n\nalpha()  # lost_widget\n",
        ),
    ]);
    let asked = [
        ("alpha", Some(SymbolKind::Class)),
        ("lost_widget", None),
        ("zz_nothing", None),
    ];
    let [kind_miss, spelled_only, nothing]: [FindResult; 3] = asked.map(|(query, kind)| {
        let limit = IntArg::from_usize(20);
        let query = query.to_owned();
        let found = find(
            &scanned.services,
            &scanned.reading(),
            &FindRequest { query, kind, limit },
        );
        found.value.expect("the scan answers")
    });
    assert!(kind_miss.declares_nothing() && !kind_miss.is_miss());
    let usage = kind_miss.usage.expect("main.py spells alpha");
    assert_eq!(rows(&usage), [("main.py", 1, 2)]);
    assert!(spelled_only.declares_nothing() && !spelled_only.is_miss());
    assert!(nothing.is_miss() && nothing.usage.is_none());
}
