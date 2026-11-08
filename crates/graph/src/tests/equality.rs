//! The equality check of a kept graph: the owned form of every part, in list order, compared part
//! by part with the parts of a full build over the same scan.

use serde_json::{Value, json};

use super::corpus::Corpus;
use super::rust::scanned;
use crate::{
    Aliases, AmbiguousDegrees, Cycle, FileFacts, NameIndex, Resolver, Scopes, SymbolGraph,
    import_cycles,
};

// Every part of one scan's graph, each by its name, in owned form.
pub(crate) struct Canonical {
    parts: Vec<(String, Value)>,
}

// The parts that one scan's graph holds, borrowed from wherever they are kept.
pub(crate) struct Parts<'p, 'a> {
    pub(crate) files: &'a [&'a FileFacts],
    pub(crate) index: &'p NameIndex<'a>,
    pub(crate) scopes: &'p Scopes<'a>,
    pub(crate) resolver: &'p Resolver<'p, 'a>,
    pub(crate) aliases: &'p Aliases<'a>,
    pub(crate) graph: &'p SymbolGraph<'a>,
    pub(crate) degrees: &'p AmbiguousDegrees,
    pub(crate) imports: &'p SymbolGraph<'a>,
    pub(crate) cycles: &'p [Cycle<'a>],
}

impl Canonical {
    pub(crate) fn of(parts: &Parts<'_, '_>) -> Canonical {
        let degrees = json!([parts.degrees.arriving, parts.degrees.leaving]);
        let cycles: Vec<Value> = (parts.cycles.iter())
            .map(|cycle| json!([cycle.files, cycle.members]))
            .collect();
        let mut forms = vec![
            ("index", parts.index.canonical()),
            ("scopes", parts.scopes.canonical(parts.files)),
            ("resolver", parts.resolver.canonical()),
            ("aliases", parts.aliases.canonical()),
            ("degrees", degrees),
            ("cycles", Value::from(cycles)),
        ];
        let mut parts_of = forms
            .drain(..)
            .map(|(name, form)| (name.to_owned(), form))
            .collect::<Vec<_>>();
        parts_of.extend(graph_parts("graph", parts.graph));
        parts_of.extend(graph_parts("imports", parts.imports));
        Canonical { parts: parts_of }
    }

    // A full build of every part of `files`.
    pub(crate) fn built(files: &[FileFacts]) -> Canonical {
        let files: Vec<&FileFacts> = files.iter().collect();
        let files = files.as_slice();
        let (index, scopes) = (NameIndex::build(files), Scopes::build(files));
        let resolver = Resolver::new(&index, &scopes);
        let graph = SymbolGraph::build(&resolver, None).expect("no deadline stops it");
        let imports = SymbolGraph::imports(&scopes);
        Canonical::of(&Parts {
            files,
            index: &index,
            scopes: &scopes,
            resolver: &resolver,
            aliases: &Aliases::build(&resolver),
            graph: &graph,
            degrees: &graph.ambiguous_degrees(),
            imports: &imports,
            cycles: &import_cycles(&imports),
        })
    }

    // Each part whose forms differ, with the first item where they part.
    pub(crate) fn differences(&self, other: &Canonical) -> Vec<String> {
        let pairs = self.parts.iter().zip(&other.parts);
        let differing = pairs.filter(|((_, left), (_, right))| left != right);
        let named = differing
            .map(|((name, left), (_, right))| format!("{name}: {}", first_difference(left, right)));
        named.collect()
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
    let full = Canonical::built(&before);
    let edited = |at: usize, text: &str| {
        let mut files = scanned(&[(before[at].path.as_str(), text)]);
        let mut after = before.clone();
        after[at] = files.remove(0);
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
    for part in [
        "index",
        "resolver",
        "graph.ambiguous",
        "graph.candidates",
        "degrees",
    ] {
        assert!(
            ambiguous.contains(&part.to_owned()),
            "{part}: {ambiguous:?}"
        );
    }
}
