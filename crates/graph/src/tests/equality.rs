//! The equality check of a kept graph: the owned form of every part, in list order, compared part
//! by part with the parts of a full build over the same scan.

use serde_json::{Value, json};

use super::corpus::Corpus;
use super::rust::scanned;
use crate::{
    Aliases, AmbiguousDegrees, Cycle, FileFacts, NameIndex, Resolver, Scopes, SymbolGraph,
    import_cycles,
};

// Every part of one scan's graph that is held, each by its name, in owned form.
pub(crate) struct Canonical {
    parts: Vec<(String, Value)>,
}

// The parts that one scan's graph holds, borrowed from wherever they are kept.
#[derive(Default)]
pub(crate) struct Parts<'p, 'a> {
    pub(crate) files: &'a [&'a FileFacts],
    pub(crate) index: Option<&'p NameIndex<'a>>,
    pub(crate) scopes: Option<&'p Scopes<'a>>,
    pub(crate) aliases: Option<&'p Aliases<'a>>,
    pub(crate) graph: Option<&'p SymbolGraph<'a>>,
    pub(crate) degrees: Option<&'p AmbiguousDegrees>,
    pub(crate) imports: Option<&'p SymbolGraph<'a>>,
    pub(crate) cycles: Option<&'p [Cycle<'a>]>,
}

impl Canonical {
    pub(crate) fn of(parts: &Parts<'_, '_>) -> Canonical {
        let mut forms: Vec<(String, Value)> = Vec::new();
        let mut put = |name: &str, form: Option<Value>| {
            forms.extend(form.map(|form| (name.to_owned(), form)));
        };
        put("index", parts.index.map(NameIndex::canonical));
        put(
            "scopes",
            parts.scopes.map(|scopes| scopes.canonical(parts.files)),
        );
        put("aliases", parts.aliases.map(Aliases::canonical));
        let degrees = |held: &AmbiguousDegrees| json!([held.arriving, held.leaving]);
        put("degrees", parts.degrees.map(degrees));
        let cycles = |held: &[Cycle<'_>]| {
            let forms = held.iter().map(|cycle| json!([cycle.files, cycle.members]));
            Value::from(forms.collect::<Vec<_>>())
        };
        put("cycles", parts.cycles.map(cycles));
        forms.extend(
            parts
                .graph
                .map(|graph| graph_parts("graph", graph))
                .into_iter()
                .flatten(),
        );
        let imports = parts.imports.map(|graph| graph_parts("imports", graph));
        forms.extend(imports.into_iter().flatten());
        Canonical { parts: forms }
    }

    // A full build of every part of `files`.
    pub(crate) fn built(files: &[&FileFacts]) -> Canonical {
        let (index, scopes) = (NameIndex::build(files), Scopes::build(files));
        let resolver = Resolver::new(&index, &scopes);
        let graph = SymbolGraph::build(&resolver, None).expect("no deadline stops it");
        let imports = SymbolGraph::imports(&scopes);
        Canonical::of(&Parts {
            files,
            index: Some(&index),
            scopes: Some(&scopes),
            aliases: Some(&Aliases::build(&resolver)),
            graph: Some(&graph),
            degrees: Some(&graph.ambiguous_degrees()),
            imports: Some(&imports),
            cycles: Some(&import_cycles(&imports)),
        })
    }

    // Each part of `self` whose form differs from the same part of `full`, with the first item
    // where they part. A part that `self` does not hold is not compared.
    pub(crate) fn differences(&self, full: &Canonical) -> Vec<String> {
        let found = |name: &str| full.parts.iter().find(|(held, _)| held == name);
        let differing = self
            .parts
            .iter()
            .filter_map(|(name, kept)| match found(name) {
                Some((_, built)) if built == kept => None,
                Some((_, built)) => Some(format!("{name}: {}", first_difference(kept, built))),
                None => Some(format!("{name}: no such part in the full build")),
            });
        differing.collect()
    }
}

// The graph's lists, each named under the part that holds it.
fn graph_parts(part: &str, graph: &SymbolGraph<'_>) -> Vec<(String, Value)> {
    let named = graph.canonical().into_iter();
    named
        .map(|(list, value)| (format!("{part}.{list}"), value))
        .collect()
}

fn first_difference(left: &Value, right: &Value) -> String {
    match (left, right) {
        (Value::Array(left), Value::Array(right)) => {
            let at = (left.iter().zip(right)).position(|(left, right)| left != right);
            match at {
                Some(at) => format!("item {at}: {} against {}", left[at], right[at]),
                None => format!("{} items against {}", left.len(), right.len()),
            }
        }
        (Value::Object(left), Value::Object(right)) => {
            let keys = left.keys().chain(right.keys());
            let key = keys
                .into_iter()
                .find(|key| left.get(*key) != right.get(*key));
            let key = key.map_or("", String::as_str);
            let (one, two) = (&left.get(key), &right.get(key));
            format!("{key}: {one:?} against {two:?}")
        }
        _ => format!("{left} against {right}"),
    }
}

// Panics with each differing part of a kept graph against a full build of the same scan.
pub(crate) fn assert_same(case: &str, kept: &Canonical, full: &Canonical) {
    let differing = kept.differences(full);
    assert!(differing.is_empty(), "{case}:\n{}", differing.join("\n"));
}

#[test]
fn two_full_builds_of_one_scan_have_one_canonical_form() {
    for corpus in Corpus::all() {
        let files = corpus.files();
        let files: Vec<&FileFacts> = files.iter().collect();
        assert_same(
            corpus.name,
            &Canonical::built(&files),
            &Canonical::built(&files),
        );
    }
}

#[test]
fn the_equality_check_names_each_part_that_one_changed_fact_changes() {
    let before = scanned(&[
        ("pkg/__init__.py", "from pkg.core import run\n"),
        (
            "pkg/core.py",
            "def run():\n    return helper()\n\n\ndef helper():\n    return 1\n",
        ),
        (
            "app.py",
            "from pkg import run\n\n\nclass App(Base):\n    def go(self):\n        run()\n        helper()\n",
        ),
        ("base.py", "class Base:\n    pass\n"),
    ]);
    let listed: Vec<&FileFacts> = before.iter().collect();
    let full = Canonical::built(&listed);
    let edited = |at: usize, text: &str| {
        let mut files = scanned(&[(before[at].path.as_str(), text)]);
        let mut after = before.clone();
        after[at] = files.remove(0);
        let after: Vec<&FileFacts> = after.iter().collect();
        Canonical::built(&after).differences(&full)
    };
    let parts = |differing: Vec<String>| -> Vec<String> {
        let names = differing
            .iter()
            .map(|line| line.split(':').next().unwrap_or_default());
        names.map(str::to_owned).collect()
    };
    let moved = parts(edited(
        1,
        "\n\ndef run():\n    return helper()\n\n\ndef helper():\n    return 1\n",
    ));
    assert!(
        moved.contains(&"index".to_owned()) && moved.contains(&"graph.edges".to_owned()),
        "{moved:?}"
    );
    let imported = parts(edited(
        2,
        "from pkg.core import run\n\n\nclass App(Base):\n    def go(self):\n        run()\n        helper()\n",
    ));
    assert!(imported.contains(&"scopes".to_owned()), "{imported:?}");
    let ambiguous = parts(edited(
        3,
        "class Base:\n    pass\n\n\ndef helper():\n    return 2\n",
    ));
    for part in ["index", "graph.ambiguous", "graph.candidates", "degrees"] {
        assert!(
            ambiguous.contains(&part.to_owned()),
            "{part}: {ambiguous:?}"
        );
    }
}
