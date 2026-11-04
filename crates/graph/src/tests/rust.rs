//! The Rust workspace binding: crate names from the Cargo manifests, the crate roots, the module
//! walk, `super` inside an inline module, the glob first hop, the module path that qualifies a
//! name, a module that a file re-exports, and the language family of a name-only match.

use std::sync::Arc;

use syntax::Language;

use crate::paths::PathIndex;
use crate::rust_paths::RustTree;
use crate::{FileFacts, NameIndex, Resolver, Scopes, Tier};

fn scanned(files: &[(&str, &str)]) -> Vec<FileFacts> {
    let each = files.iter().map(|(path, text)| {
        let language = Language::from_path(path).expect("a known language");
        let parsed = syntax::parse(language, Arc::from(*text)).expect("the text parses");
        FileFacts {
            path: (*path).to_owned(),
            language,
            line_count: 1,
            facts: facts::extract(&parsed),
        }
    });
    each.collect()
}

// The tier and the candidate paths of `name` as the file at `path` spells it.
fn resolved(files: &[FileFacts], name: &str, path: &str) -> Option<(Tier, Vec<String>)> {
    let (index, scopes) = (NameIndex::build(files), Scopes::build(files));
    let found = Resolver::new(&index, &scopes).resolve(name, path)?;
    let paths = found.candidates.iter().map(|entry| entry.path().to_owned());
    Some((found.tier, paths.collect()))
}

// The tier and candidate paths of each occurrence of `name` in the file at `path` that binds.
fn sites(files: &[FileFacts], name: &str, path: &str) -> Vec<(Tier, Vec<String>)> {
    let (index, scopes) = (NameIndex::build(files), Scopes::build(files));
    let resolver = Resolver::new(&index, &scopes);
    let file = files
        .iter()
        .find(|file| file.path == path)
        .expect("a scanned file");
    let found = resolver.resolved_sites(file, None);
    let named = found.filter(|(site, _)| site.name == name);
    let rows = named.map(|(_, resolution)| {
        let paths = resolution
            .candidates
            .iter()
            .map(|entry| entry.path().to_owned());
        (resolution.tier, paths.collect())
    });
    rows.collect()
}

fn imported(paths: &[&str]) -> (Tier, Vec<String>) {
    let paths = paths.iter().map(|path| (*path).to_owned());
    (Tier::Imported, paths.collect())
}

fn import_pairs(files: &[FileFacts]) -> Vec<(String, String)> {
    let scopes = Scopes::build(files);
    let pairs = scopes.import_pairs().into_iter();
    pairs
        .map(|(from, to)| (from.to_owned(), to.to_owned()))
        .collect()
}

const WORKSPACE: [(&str, &str); 7] = [
    ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
    (
        "crates/graph-core/Cargo.toml",
        "[package]\nname = \"graph-core\"\n",
    ),
    ("crates/graph-core/src/lib.rs", "pub mod rank;\n"),
    ("crates/graph-core/src/rank.rs", "pub fn pagerank() {}\n"),
    (
        "crates/app/Cargo.toml",
        "[package]\nname = \"app\"\n\n[lib]\nname = \"front\"\n",
    ),
    ("crates/app/src/lib.rs", "mod map;\n"),
    (
        "crates/app/src/map.rs",
        "use graph_core::rank::pagerank;\n\nfn draw() {\n    pagerank();\n}\n",
    ),
];

#[test]
fn a_use_of_a_workspace_crate_binds_through_its_manifest_name() {
    let files = scanned(&WORKSPACE);
    let rank = "crates/graph-core/src/rank.rs";
    assert_eq!(
        resolved(&files, "pagerank", "crates/app/src/map.rs"),
        Some(imported(&[rank]))
    );
    let pair = ("crates/app/src/map.rs".to_owned(), rank.to_owned());
    assert!(import_pairs(&files).contains(&pair));
}

#[test]
fn without_its_manifest_a_crate_name_binds_nothing() {
    let files: Vec<FileFacts> = (scanned(&WORKSPACE).into_iter())
        .filter(|file| file.path != "crates/graph-core/Cargo.toml")
        .collect();
    let found = resolved(&files, "pagerank", "crates/app/src/map.rs");
    assert_eq!(found.map(|(tier, _)| tier), Some(Tier::Unique));
}

#[test]
fn a_lib_name_and_a_lib_path_name_the_crate_and_its_root() {
    let files = scanned(&[
        (
            "Cargo.toml",
            "[package]\nname = \"pkg\"\n[lib]\nname = \"core_lib\"\npath = \"lib/core.rs\"\n",
        ),
        ("lib/core.rs", "pub fn shared() {}\n"),
        (
            "src/main.rs",
            "use core_lib::shared;\nfn main() { shared(); }\n",
        ),
    ]);
    assert_eq!(
        resolved(&files, "shared", "src/main.rs"),
        Some(imported(&["lib/core.rs"]))
    );
}

#[test]
fn each_target_file_finds_its_crate_root() {
    let files = scanned(&[
        ("Cargo.toml", "[package]\nname = \"p\"\n"),
        ("src/lib.rs", ""),
        ("src/a/b.rs", ""),
        ("src/main.rs", ""),
        ("src/bin/tool.rs", ""),
        ("src/bin/multi/main.rs", ""),
        ("src/bin/multi/util.rs", ""),
        ("tests/it.rs", ""),
        ("examples/demo/extra.rs", ""),
        ("build.rs", ""),
        ("member/Cargo.toml", "[package]\nname = \"m\"\n"),
        ("member/src/lib.rs", ""),
        ("member/src/x.rs", ""),
    ]);
    let paths = PathIndex::build(files.iter().map(|file| file.path.as_str()));
    let tree = RustTree::build(&files, &paths);
    let roots = [
        ("src/lib.rs", Some("src/lib.rs")),
        ("src/a/b.rs", Some("src/lib.rs")),
        ("src/main.rs", Some("src/main.rs")),
        ("src/bin/tool.rs", Some("src/bin/tool.rs")),
        ("src/bin/multi/util.rs", Some("src/bin/multi/main.rs")),
        ("tests/it.rs", Some("tests/it.rs")),
        ("examples/demo/extra.rs", None),
        ("build.rs", Some("build.rs")),
        ("member/src/x.rs", Some("member/src/lib.rs")),
    ];
    for (path, root) in roots {
        let held = files.iter().find(|file| file.path == path).expect("a file");
        assert_eq!(tree.crate_root(&held.path), root, "{path}");
    }
}

const MODULES: [(&str, &str); 8] = [
    ("Cargo.toml", "[package]\nname = \"p\"\n"),
    ("src/lib.rs", "pub mod a;\npub mod c;\npub fn top() {}\n"),
    ("src/a.rs", "pub mod b;\n"),
    ("src/a/b.rs", "pub fn deep() {}\n"),
    ("src/c/mod.rs", "pub mod d;\n"),
    (
        "src/c/d.rs",
        "use super::super::a::b::deep;\nuse crate::top;\nuse self::e::Gone;\n\
         fn f() { deep(); top(); }\n",
    ),
    ("src/x.rs", "pub fn top() {}\n"),
    ("src/y.rs", "pub fn deep() {}\n"),
];

#[test]
fn crate_self_and_super_walk_the_module_files() {
    let files = scanned(&MODULES);
    assert_eq!(
        resolved(&files, "deep", "src/c/d.rs"),
        Some(imported(&["src/a/b.rs"]))
    );
    assert_eq!(
        resolved(&files, "top", "src/c/d.rs"),
        Some(imported(&["src/lib.rs"]))
    );
    let scopes = Scopes::build(&files);
    let scope = scopes.get("src/c/d.rs").expect("a scope");
    let rows: Vec<(&str, Option<&str>)> = (scope.statements.iter())
        .map(|row| (row.module.as_ref(), row.resolved))
        .collect();
    let expected = [
        ("super::super::a::b::deep", Some("src/a/b.rs")),
        ("crate::top", Some("src/lib.rs")),
        ("self::e::Gone", Some("src/c/d.rs")),
    ];
    assert_eq!(rows, expected);
}

#[test]
fn a_use_that_names_no_repository_module_keeps_one_unresolved_row() {
    let files = scanned(&[
        ("Cargo.toml", "[package]\nname = \"p\"\n"),
        ("src/lib.rs", "use std::{fmt, io::Read};\n"),
    ]);
    let scopes = Scopes::build(&files);
    let scope = scopes.get("src/lib.rs").expect("a scope");
    let rows: Vec<(&str, Option<&str>)> = (scope.statements.iter())
        .map(|row| (row.module.as_ref(), row.resolved))
        .collect();
    assert_eq!(rows, [("std", None)]);
}

#[test]
fn super_inside_an_inline_module_stays_in_the_file_and_a_further_super_leaves_it() {
    let files = scanned(&[
        ("Cargo.toml", "[package]\nname = \"p\"\n"),
        ("src/lib.rs", "pub mod a;\npub fn root() {}\n"),
        (
            "src/a.rs",
            "pub fn here() {}\n\nmod tests {\n    use super::here;\n    use super::super::root;\n}\n",
        ),
    ]);
    let pairs = import_pairs(&files);
    assert_eq!(pairs, [("src/a.rs".to_owned(), "src/lib.rs".to_owned())]);
    let scopes = Scopes::build(&files);
    let scope = scopes.get("src/a.rs").expect("a scope");
    let rows: Vec<Option<&str>> = scope.statements.iter().map(|row| row.resolved).collect();
    assert_eq!(rows, [Some("src/a.rs"), Some("src/lib.rs")]);
}

#[test]
fn a_glob_reaches_what_the_globbed_module_re_exports() {
    let files = scanned(&[
        ("Cargo.toml", "[package]\nname = \"p\"\n"),
        ("src/lib.rs", "mod a;\nmod b;\nmod c;\n"),
        ("src/a.rs", "pub use crate::b::deep;\n"),
        ("src/b.rs", "pub fn deep() {}\n"),
        ("src/c.rs", "use crate::a::*;\n\nfn f() {\n    deep();\n}\n"),
        ("src/z.rs", "pub fn deep() {}\n"),
    ]);
    assert_eq!(
        resolved(&files, "deep", "src/c.rs"),
        Some(imported(&["src/b.rs"]))
    );
}

const QUALIFIED: [(&str, &str); 5] = [
    ("Cargo.toml", "[package]\nname = \"p\"\n"),
    ("src/lib.rs", "pub mod util;\npub fn path() {}\n"),
    ("src/util.rs", "pub fn helper() {}\n"),
    ("src/other.rs", "pub fn helper() {}\npub fn gone() {}\n"),
    (
        "src/x.rs",
        "fn f() {\n    crate::util::helper();\n    std::path::PathBuf::new();\n    \
         crate::nomod::gone();\n}\n",
    ),
];

#[test]
fn a_module_path_binds_its_name_through_the_module_and_a_foreign_path_binds_nothing() {
    let files = scanned(&QUALIFIED);
    assert_eq!(
        sites(&files, "helper", "src/x.rs"),
        [(Tier::Imported, vec!["src/util.rs".to_owned()])]
    );
    assert_eq!(sites(&files, "path", "src/x.rs"), []);
    assert_eq!(
        sites(&files, "gone", "src/x.rs"),
        [(Tier::Unique, vec!["src/other.rs".to_owned()])]
    );
}

#[test]
fn a_path_through_an_inline_or_a_path_attribute_module_falls_back_to_the_name() {
    let files = scanned(&[
        ("Cargo.toml", "[package]\nname = \"p\"\n"),
        (
            "src/lib.rs",
            "#[path = \"platform_impl.rs\"]\nmod platform;\n\nmod m {\n    pub fn f() {}\n}\n\n\
             fn g() {\n    m::f();\n    platform::tick();\n}\n",
        ),
        ("src/platform_impl.rs", "pub fn tick() {}\n"),
    ]);
    assert_eq!(
        sites(&files, "f", "src/lib.rs"),
        [(Tier::SameFile, vec!["src/lib.rs".to_owned()])]
    );
    assert_eq!(
        sites(&files, "tick", "src/lib.rs"),
        [(Tier::Unique, vec!["src/platform_impl.rs".to_owned()])]
    );
}

#[test]
fn a_super_path_inside_an_inline_module_binds_the_item_of_its_own_file() {
    let files = scanned(&[
        ("Cargo.toml", "[package]\nname = \"p\"\n"),
        ("src/lib.rs", "pub mod a;\npub fn helper() {}\n"),
        (
            "src/a.rs",
            "pub fn helper() {}\n\nmod tests {\n    fn t() {\n        super::helper();\n    }\n}\n",
        ),
    ]);
    assert_eq!(
        sites(&files, "helper", "src/a.rs"),
        [(Tier::SameFile, vec!["src/a.rs".to_owned()])]
    );
}

#[test]
fn a_leading_double_colon_names_a_workspace_crate() {
    let mut files = WORKSPACE;
    files[6].1 = "use ::graph_core::rank::pagerank;\n\nfn draw() {\n    pagerank();\n}\n";
    assert_eq!(
        resolved(&scanned(&files), "pagerank", "crates/app/src/map.rs"),
        Some(imported(&["crates/graph-core/src/rank.rs"]))
    );
}

#[test]
fn a_module_that_defines_no_such_name_falls_back_to_the_name() {
    let files = scanned(&[
        ("Cargo.toml", "[package]\nname = \"p\"\n"),
        ("src/lib.rs", "pub mod util;\n"),
        ("src/util.rs", "pub fn helper() {}\n"),
        ("src/other.rs", "pub fn gone() {}\n"),
        ("src/x.rs", "fn f() {\n    crate::util::gone();\n}\n"),
    ]);
    assert_eq!(
        sites(&files, "gone", "src/x.rs"),
        [(Tier::Unique, vec!["src/other.rs".to_owned()])]
    );
}

const UNRESOLVED_USE: [(&str, &str); 4] = [
    ("Cargo.toml", "[package]\nname = \"p\"\n"),
    ("src/lib.rs", "pub mod a;\n"),
    ("src/a.rs", "pub fn f() {}\n"),
    ("src/other.rs", "pub fn helper() {}\n"),
];

#[test]
fn a_qualifier_that_an_unresolved_use_of_this_repository_binds_falls_back_to_the_name() {
    let mut files = UNRESOLVED_USE.to_vec();
    files.push((
        "src/x.rs",
        "use crate::gone::deep::util;\n\nfn g() {\n    util::helper();\n}\n",
    ));
    assert_eq!(
        sites(&scanned(&files), "helper", "src/x.rs"),
        [(Tier::Unique, vec!["src/other.rs".to_owned()])]
    );
}

// A macro body is a token tree, so only a top-level `use $crate::...`, which rustc rejects, parses.
#[test]
fn dollar_crate_names_the_crate_root() {
    let mut files = UNRESOLVED_USE.to_vec();
    files.push((
        "src/y.rs",
        "use $crate::a::f;\nuse $crate::gone::kit;\n\nfn h() {\n    f();\n    kit::helper();\n}\n",
    ));
    let files = scanned(&files);
    assert_eq!(
        resolved(&files, "f", "src/y.rs"),
        Some(imported(&["src/a.rs"]))
    );
    assert_eq!(
        sites(&files, "helper", "src/y.rs"),
        [(Tier::Unique, vec!["src/other.rs".to_owned()])]
    );
}

#[test]
fn a_segment_may_name_a_module_that_a_file_re_exports_or_an_inline_module_directory() {
    let files = scanned(&[
        ("Cargo.toml", "[package]\nname = \"p\"\n"),
        (
            "src/lib.rs",
            "pub mod prompts;\nmod outer {\n    pub mod inner;\n}\n",
        ),
        (
            "src/prompts.rs",
            "pub mod generated;\npub use generated::messages;\n",
        ),
        ("src/prompts/generated/mod.rs", "pub mod messages;\n"),
        ("src/prompts/generated/messages.rs", "pub fn render() {}\n"),
        ("src/outer/inner.rs", "pub fn nested() {}\n"),
        (
            "src/x.rs",
            "use crate::prompts::messages::render;\nuse crate::outer::inner::nested;\n",
        ),
        ("src/y.rs", "pub fn render() {}\npub fn nested() {}\n"),
    ]);
    assert_eq!(
        resolved(&files, "render", "src/x.rs"),
        Some(imported(&["src/prompts/generated/messages.rs"]))
    );
    assert_eq!(
        resolved(&files, "nested", "src/x.rs"),
        Some(imported(&["src/outer/inner.rs"]))
    );
}

#[test]
fn a_name_only_match_keeps_the_definitions_of_the_spelling_language_family() {
    let files = scanned(&[
        ("a.py", "def shared():\n    pass\n"),
        ("b.rs", "fn shared() {}\n"),
        ("c.ts", "export function shared() {}\n"),
        ("d.js", "shared();\n"),
        ("e.lua", "x = shared\n"),
        ("f.py", "shared()\n"),
    ]);
    let unique = |path: &str| Some((Tier::Unique, vec![path.to_owned()]));
    assert_eq!(resolved(&files, "shared", "d.js"), unique("c.ts"));
    assert_eq!(resolved(&files, "shared", "f.py"), unique("a.py"));
    assert_eq!(resolved(&files, "shared", "e.lua"), None);
}
