//! The facts that the Rust workspace binding reads: the use trees, the inline-module depth of a
//! `use`, the module path that qualifies a name, the `mod` items of a file, the lifetimes and
//! labels that name nothing, and the Cargo manifest of a TOML document.

use std::sync::Arc;

use facts::{Facts, Manifest, extract};
use syntax::{Language, parse};

// Each member path below the module with the local name it binds.
type Members = Vec<(String, String)>;

fn facts_of(language: Language, source: &str) -> Facts {
    extract(&parse(language, Arc::from(source)).expect("the source parses"))
}

// Each import as (module, member and local name pairs, binds_all, inline depth).
fn imports(source: &str) -> Vec<(String, Members, bool, u32)> {
    let found = facts_of(Language::Rust, source).imports;
    let rows = found.iter().map(|import| {
        let pairs = import.bound_names();
        let pairs = pairs.map(|(member, local)| (member.to_owned(), local.to_owned()));
        let row = (import.module.clone(), pairs.collect(), import.binds_all);
        (row.0, row.1, row.2, import.inline_depth)
    });
    rows.collect()
}

fn pairs(rows: &[(&str, &str)]) -> Members {
    let owned = rows
        .iter()
        .map(|(member, local)| ((*member).to_owned(), (*local).to_owned()));
    owned.collect()
}

#[test]
fn a_use_tree_gives_one_import_with_each_leaf_as_a_member_path_and_its_local_name() {
    let source = "use a::b::c;\nuse a::b as d;\nuse a::{b, c::{d, e as f}, g::*, self};\n\
                  use a::*;\nuse {x, y::z};\nuse std;\n";
    let expected = [
        ("a::b", pairs(&[("c", "c")]), false),
        ("a", pairs(&[("b", "d")]), false),
        (
            "a",
            pairs(&[
                ("b", "b"),
                ("c::d", "d"),
                ("c::e", "f"),
                ("g::*", "g::*"),
                ("self", "self"),
            ]),
            false,
        ),
        ("a", Vec::new(), true),
        ("", pairs(&[("x", "x"), ("y::z", "z")]), false),
        ("std", Vec::new(), false),
    ];
    let found: Vec<(String, Members, bool)> = imports(source)
        .into_iter()
        .map(|(module, members, binds_all, _)| (module, members, binds_all))
        .collect();
    let expected: Vec<(String, Members, bool)> = expected
        .into_iter()
        .map(|(module, members, binds_all)| (module.to_owned(), members, binds_all))
        .collect();
    assert_eq!(found, expected);
}

#[test]
fn the_inline_depth_counts_the_mod_bodies_around_a_use_and_not_a_function_body() {
    let source = "use super::a;\nmod outer {\n    use super::b;\n    fn f() { use super::c; }\n    \
                  mod inner {\n        use super::super::d;\n    }\n}\n";
    let depths: Vec<(String, u32)> = imports(source)
        .into_iter()
        .map(|(_, members, _, depth)| (members[0].0.clone(), depth))
        .collect();
    let expected = [("a", 0), ("b", 1), ("c", 1), ("d", 2)];
    let expected: Vec<(String, u32)> = expected
        .iter()
        .map(|(member, depth)| ((*member).to_owned(), *depth))
        .collect();
    assert_eq!(depths, expected);
}

// Each reference as (name, qualifier), in document order.
fn qualified(source: &str) -> Vec<(String, String)> {
    let refs = facts_of(Language::Rust, source).refs;
    let rows = refs
        .iter()
        .map(|found| (found.name.clone(), found.qualifier.clone()));
    rows.collect()
}

#[test]
fn a_module_path_qualifies_the_segment_after_it_and_a_type_path_qualifies_nothing() {
    let source = "fn f() { a::b::c(); crate::m::T::new(); Vec::new(); self::g(); }\n";
    let expected = [
        ("f", ""),
        ("a", ""),
        ("b", "a"),
        ("c", "a::b"),
        ("m", "crate"),
        ("T", "crate::m"),
        ("new", ""),
        ("Vec", ""),
        ("new", ""),
        ("g", "self"),
    ];
    let expected: Vec<(String, String)> = expected
        .iter()
        .map(|(name, qualifier)| ((*name).to_owned(), (*qualifier).to_owned()))
        .collect();
    assert_eq!(qualified(source), expected);
}

#[test]
fn a_self_or_super_path_inside_an_inline_module_qualifies_nothing_and_the_mod_items_are_listed() {
    let source = "fn f() { m::g(); self::h(); }\nmod m {\n    pub fn g() { super::h(); self::g(); \
                  crate::k(); }\n    mod inner {}\n}\n#[path = \"x.rs\"]\nmod n;\n";
    let expected = [
        ("f", ""),
        ("m", ""),
        ("g", "m"),
        ("h", "self"),
        ("m", ""),
        ("g", ""),
        ("h", ""),
        ("g", ""),
        ("k", "crate"),
        ("inner", ""),
        ("path", ""),
        ("n", ""),
    ];
    let expected: Vec<(String, String)> = expected
        .iter()
        .map(|(name, qualifier)| ((*name).to_owned(), (*qualifier).to_owned()))
        .collect();
    assert_eq!(qualified(source), expected);
    let modules = facts_of(Language::Rust, source).modules;
    assert_eq!(modules, ["m", "inner", "n"]);
    assert!(facts_of(Language::Python, "import m\n").modules.is_empty());
}

#[test]
fn a_lifetime_or_a_label_is_no_reference() {
    let source = "fn f<'a>(x: &'a Text) -> &'a Text { 'outer: loop { break 'outer; } }\n";
    let names: Vec<String> = qualified(source)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, ["f", "x", "Text", "Text"]);
}

#[test]
fn a_python_reference_keeps_no_rust_qualifier() {
    let refs = facts_of(Language::Python, "import a\na.b.c()\n").refs;
    let qualifiers: Vec<(&str, &str)> = (refs.iter())
        .map(|found| (found.name.as_str(), found.qualifier.as_str()))
        .collect();
    assert_eq!(qualifiers, [("a", ""), ("a", ""), ("b", "a"), ("c", "")]);
}

fn manifest(source: &str) -> Option<Manifest> {
    facts_of(Language::Toml, source).manifest
}

#[test]
fn a_cargo_manifest_gives_its_package_and_library_names_and_the_library_path() {
    let source = "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n\n[lib]\nname = 'core'\n\
                  path = \"lib/core.rs\"\n\n[dependencies]\nname = \"other\"\n";
    let expected = Manifest {
        package: "my-crate".to_owned(),
        lib_name: Some("core".to_owned()),
        lib_path: Some("lib/core.rs".to_owned()),
    };
    assert_eq!(manifest(source), Some(expected));
    let plain = Manifest {
        package: "plain".to_owned(),
        lib_name: None,
        lib_path: None,
    };
    assert_eq!(manifest("[package]\nname = \"plain\"\n"), Some(plain));
}

#[test]
fn a_document_without_a_package_name_has_no_manifest_and_a_value_that_is_no_plain_string_is_absent()
{
    assert_eq!(manifest("[workspace]\nmembers = [\"a\"]\n"), None);
    assert_eq!(manifest("name = \"top\"\n[project]\nname = \"py\"\n"), None);
    assert_eq!(manifest("[package]\nname = 3\n"), None);
    assert_eq!(manifest("[package]\nname.workspace = true\n"), None);
    assert_eq!(manifest("[package]\nname = \"\"\n"), None);
    let source = "[package]\nname = \"p\"\n[lib]\nname = \"a\\\\b\"\npath = \"\"\"x.rs\"\"\"\n";
    let expected = Manifest {
        package: "p".to_owned(),
        lib_name: None,
        lib_path: None,
    };
    assert_eq!(manifest(source), Some(expected));
    assert_eq!(
        facts_of(Language::Json, "{\"package\": {\"name\": \"x\"}}").manifest,
        None
    );
}
