use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;
use text::testing::document;

use crate::generated::languages::PROBE_ROOTS;
use crate::tables::Measured;
use crate::{EXTENSIONS, Language, NodeId, Parsed, Tier, parse, rust_impl_owner};

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
        for (node, rule, expected) in &case.results {
            let found = rule_result(&parsed, NodeId(*node), rule);
            assert_eq!(&found, expected, "{} node {node}: {rule}", case.name);
        }
    }
}

fn rule_result(parsed: &Parsed, node: NodeId, rule: &str) -> Value {
    let spec = parsed.language().spec().expect("a tier-1 language");
    let index = |found: Option<NodeId>| Value::from(found.map_or(-1, |found| i64::from(found.0)));
    match rule {
        "name_of" => index(spec.units.name_of(parsed, node)),
        "binding_of" => index(spec.units.binding_of(parsed, node)),
        "owner_of" => index(spec.units.owner_of(parsed, node)),
        "rust_impl_owner" => index(rust_impl_owner(parsed, node)),
        "condition" => Value::from(spec.decisions.condition(parsed, node).expect("a test")),
        "skips" => Value::from(spec.clones.skips(parsed, node)),
        other => panic!("the fixture names an unknown rule {other}"),
    }
}
