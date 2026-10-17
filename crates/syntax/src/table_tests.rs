use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use text::testing::document;

use crate::generated::languages::PROBE_ROOTS;
use crate::tables::Measured;
use crate::{
    EXTENSIONS, FieldCursor, ImportRule, Language, NodeId, SymbolRule, TABLES_DIGEST, Tier, parse,
    rust_impl_owner,
};

#[derive(Deserialize)]
struct Source {
    name: String,
    source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleCase {
    name: String,
    language: String,
    results: Vec<(u32, String, Value)>,
}

fn table_names(measured: &Measured) -> Vec<&'static str> {
    let (units, decisions, clones) = (&measured.units, &measured.decisions, &measured.clones);
    let mut names: Vec<&'static str> = [
        measured.statement_parents.unwrap_or_default(),
        units.units,
        units.scopes,
        units.bound,
        units.descend.unwrap_or_default(),
        decisions.points,
        clones.functions,
        clones.blocks,
        clones.identifiers,
        clones.indent_blocks,
    ]
    .concat();
    names.extend(decisions.conditional.iter().map(|(kind, _)| *kind));
    for (kind, tokens) in decisions.token_operators {
        names.push(kind);
        names.extend(*tokens);
    }
    names.extend(decisions.field_operators.iter().map(|(kind, _)| *kind));
    names.extend(
        clones
            .literals
            .iter()
            .flat_map(|(_, kinds)| kinds.iter().copied()),
    );
    names
}

#[test]
fn every_kind_that_a_table_names_exists_in_its_grammar() {
    for language in Language::ALL {
        for name in language.table().comments {
            assert!(
                !language.kinds(&[name]).is_empty(),
                "{language:?} has no kind {name}"
            );
        }
        let Some(measured) = language.table().measured else {
            continue;
        };
        for name in table_names(measured) {
            assert!(
                !language.kinds(&[name]).is_empty(),
                "{language:?} has no kind {name}"
            );
        }
        let operators = !measured.decisions.field_operators.is_empty();
        let field = language
            .spec()
            .and_then(|spec| spec.decisions.operator_field());
        assert!(
            !operators || field.is_some(),
            "{language:?} has no operator field"
        );
    }
}

#[test]
fn every_language_has_extraction_tables_under_a_distinct_prefix() {
    let mut prefixes: Vec<&str> = Language::ALL
        .iter()
        .map(|language| language.extraction().prefix)
        .collect();
    assert!(prefixes.iter().all(|prefix| !prefix.is_empty()));
    prefixes.sort_unstable();
    prefixes.dedup();
    assert_eq!(prefixes.len(), Language::ALL.len());
    for language in Language::ALL {
        let table = language.table().extraction;
        for name in [table.bodies, table.skeleton_functions].concat() {
            assert!(
                !language.kinds(&[name]).is_empty(),
                "{language:?} has no kind {name}"
            );
        }
        let extraction = language.extraction();
        let generic =
            extraction.symbols == SymbolRule::Generic || extraction.imports == ImportRule::Generic;
        assert!(!generic || table.declarations.is_some(), "{language:?}");
    }
}

#[test]
fn only_python_has_builtins_and_they_are_sorted() {
    let builtins = Language::Python.table().extraction.builtins;
    assert_eq!(builtins.len(), 191);
    assert!(builtins.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(Language::Python.extraction().is_builtin("len"));
    assert!(!Language::Python.extraction().is_builtin("length"));
    for language in Language::ALL {
        let builtins = language.table().extraction.builtins;
        assert_eq!(
            builtins.is_empty(),
            language != Language::Python,
            "{language:?}"
        );
    }
}

#[test]
fn keyword_visibility_follows_the_listed_modifier_kinds_not_the_grammar() {
    let javascript = &Language::Javascript.extraction().declarations;
    assert!(javascript.keyword_visibility);
    assert!(javascript.modifiers.is_empty());
    assert!(Language::Java.extraction().declarations.keyword_visibility);
    assert!(!Language::Go.extraction().declarations.keyword_visibility);
    assert!(
        !Language::Python
            .extraction()
            .declarations
            .keyword_visibility
    );
}

#[test]
fn each_grammar_parses_its_probe_into_the_root_that_the_python_bundle_gives() {
    for (language, root) in Language::ALL.into_iter().zip(PROBE_ROOTS) {
        let parsed = parse(language, Arc::from(language.probe_sample())).expect("it parses");
        assert!(!parsed.has_error(parsed.root()), "{language:?}");
        assert!(parsed.child_count(parsed.root()) > 0, "{language:?}");
        assert_eq!(parsed.kind_name(parsed.root()), root, "{language:?}");
    }
}

#[test]
fn the_twelve_tier_one_languages_have_tables() {
    let tier_one: Vec<&str> = Language::ALL
        .into_iter()
        .filter(|language| language.tier() == Tier::One)
        .map(Language::name)
        .collect();
    assert_eq!(
        tier_one,
        [
            "bash",
            "c",
            "cpp",
            "go",
            "java",
            "javascript",
            "lua",
            "python",
            "ruby",
            "rust",
            "tsx",
            "typescript"
        ]
    );
    for language in Language::ALL {
        assert_eq!(
            language.spec().is_some(),
            language.tier() == Tier::One,
            "{language:?}"
        );
        assert_eq!(
            language.tier().number(),
            if language.spec().is_some() { 1 } else { 2 }
        );
    }
}

#[test]
fn the_tables_digest_is_the_sha256_of_the_compiled_tables() {
    let fresh = Sha256::digest(include_bytes!("generated/languages.rs"))
        .iter()
        .fold(String::new(), |mut hex, byte| {
            write!(hex, "{byte:02x}").expect("a String takes every write");
            hex
        });
    assert_eq!(TABLES_DIGEST, fresh, "run cargo xtask generate");
}

#[test]
fn the_extension_map_holds_each_language_suffix_once_in_order() {
    assert_eq!(EXTENSIONS.len(), 33);
    assert!(EXTENSIONS.windows(2).all(|pair| pair[0].0 < pair[1].0));
    for (suffix, language) in EXTENSIONS {
        assert!(language.extensions().contains(&suffix), "{suffix}");
    }
}

#[test]
fn a_path_selects_its_language_by_the_last_suffix_of_its_final_component() {
    let cases = [
        ("a.py", Some(Language::Python)),
        ("src/x.tar.py", Some(Language::Python)),
        ("a..py", Some(Language::Python)),
        ("a.py/", Some(Language::Python)),
        ("dir/view.tsx", Some(Language::Tsx)),
        ("lib.h", Some(Language::C)),
        ("main.tf", Some(Language::Hcl)),
        ("A.PY", None),
        (".py", None),
        ("x.", None),
        ("d.py/file", None),
        ("Makefile", None),
        ("", None),
    ];
    for (path, expected) in cases {
        assert_eq!(Language::from_path(path), expected, "{path}");
    }
    for (suffix, language) in EXTENSIONS {
        assert_eq!(
            Language::from_path(&format!("dir/file{suffix}")),
            Some(language)
        );
    }
}

#[test]
fn every_rule_returns_what_the_reference_rule_returns() {
    let sources: BTreeMap<String, String> =
        document::<Vec<Source>>(env!("CARGO_MANIFEST_DIR"), "parse")
            .into_iter()
            .map(|source| (source.name, source.source))
            .collect();
    let cases: Vec<RuleCase> = document(env!("CARGO_MANIFEST_DIR"), "rules");
    let checked: usize = cases.iter().map(|case| case.results.len()).sum();
    assert!(checked > 3000, "the fixture holds {checked} results");
    for case in &cases {
        let language = Language::from_name(&case.language).expect("a known language");
        let parsed = parse(language, Arc::from(sources[&case.name].as_str())).expect("it parses");
        let mut fields = FieldCursor::new(&parsed);
        for (node, rule, expected) in &case.results {
            let found = rule_result(&mut fields, NodeId(*node), rule);
            assert_eq!(&found, expected, "{} node {node}: {rule}", case.name);
        }
    }
}

fn rule_result(fields: &mut FieldCursor<'_>, node: NodeId, rule: &str) -> Value {
    let parsed = fields.parsed();
    let spec = parsed.language().spec().expect("a tier-1 language");
    let index = |found: Option<NodeId>| Value::from(found.map_or(-1, |found| i64::from(found.0)));
    match rule {
        "name_of" => index(spec.units.name_of(fields, node)),
        "binding_of" => index(spec.units.binding_of(fields, node)),
        "owner_of" => index(spec.units.owner_of(fields, node)),
        "rust_impl_owner" => index(rust_impl_owner(fields, node)),
        "condition" => Value::from(spec.decisions.condition(fields, node).expect("a test")),
        "skips" => Value::from(spec.clones.skips(parsed, node)),
        other => panic!("the fixture names an unknown rule {other}"),
    }
}
