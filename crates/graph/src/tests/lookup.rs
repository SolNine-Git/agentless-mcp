use std::borrow::Cow;

use facts::StableId;
use serde_json::{Value, json};

use super::corpus::{Corpus, ids};
use crate::lookup::{Target, TargetForm, lookup_target, parse_target, path_name_id};
use crate::names::NameIndex;

fn id(text: &str) -> StableId {
    text.parse().unwrap_or_else(|error| panic!("{error}"))
}

fn named(text: &str) -> Target<'_> {
    Target::Named(Cow::Borrowed(text))
}

#[test]
fn each_form_parses_to_its_own_target() {
    let cases = [
        (
            "py:pricing.py::format_money",
            Target::Id(id("py:pricing.py::format_money"), TargetForm::StableId),
        ),
        (
            "pricing.py::PriceBook.quote",
            Target::Id(id("py:pricing.py::PriceBook.quote"), TargetForm::PathName),
        ),
        (
            "crates/json/src/float.rs::float_text",
            Target::Id(
                id("rs:crates/json/src/float.rs::float_text"),
                TargetForm::PathName,
            ),
        ),
        ("pricing.py:40", Target::Line("pricing.py", 40)),
        ("Redirect::permanent", named("Redirect.permanent")),
        ("json::float::float_text", named("json.float.float_text")),
        ("PriceBook.quote", named("PriceBook.quote")),
        ("format_money", named("format_money")),
        ("::x", named("::x")),
        ("py:", named("py:")),
        ("notes.txt:3", named("notes.txt:3")),
        ("pricing.py:", named("pricing.py:")),
    ];
    for (text, expected) in cases {
        assert_eq!(parse_target(text), expected, "{text:?}");
    }
}

#[test]
fn a_path_name_takes_the_prefix_of_its_language_and_nothing_else_does() {
    let spelled = |text: &str| path_name_id(text).map(|id| id.to_string());
    assert_eq!(
        spelled("src/app.tsx::App.render").as_deref(),
        Some("tsx:src/app.tsx::App.render")
    );
    assert_eq!(spelled("Makefile::all"), None);
    assert_eq!(spelled("py:a.py::f"), None);
    assert_eq!(spelled("a.py::"), None);
    assert_eq!(spelled("format_money"), None);
}

#[test]
fn the_last_name_of_each_form_is_the_name_its_definitions_carry() {
    let last = |text: &str| parse_target(text).last_name().map(str::to_owned);
    assert_eq!(
        last("py:pricing.py::PriceBook.quote#2").as_deref(),
        Some("quote")
    );
    assert_eq!(last("Type::method").as_deref(), Some("method"));
    assert_eq!(last("format_money").as_deref(), Some("format_money"));
    assert_eq!(last("pricing.py:40"), None);
}

#[test]
fn each_form_keeps_its_own_scope() {
    let corpus = Corpus::load("repo_py");
    let files = corpus.files();
    let index = NameIndex::build(&files);
    let looked = |text: &str| {
        let found = lookup_target(&index, &parse_target(text));
        (ids(&found.definitions), found.scoped, found.form)
    };
    let cases: [(&str, Value, bool, TargetForm); 7] = [
        (
            "pricing.py::format_money",
            json!(["py:pricing.py::format_money"]),
            true,
            TargetForm::PathName,
        ),
        (
            "gone.py::format_money",
            json!(["py:pricing.py::format_money"]),
            false,
            TargetForm::PathName,
        ),
        (
            "pricing.py:40",
            json!(["py:pricing.py::PriceBook.cost_of"]),
            true,
            TargetForm::PathLine,
        ),
        ("pricing.py:1", json!([]), false, TargetForm::PathLine),
        ("pricing.py", json!([]), false, TargetForm::Path),
        (
            "PriceBook::cost_of",
            json!(["py:pricing.py::PriceBook.cost_of"]),
            false,
            TargetForm::QualifiedName,
        ),
        ("zz_nothing", json!([]), true, TargetForm::Name),
    ];
    for (text, definitions, scoped, form) in cases {
        assert_eq!(looked(text), (definitions, scoped, form), "{text:?}");
    }
}
