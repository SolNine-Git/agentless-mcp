use std::collections::{BTreeMap, BTreeSet};

use syntax::Language;

use super::{Link, NamedFunction, NamedTree, reuse_candidates};
use crate::{Detail, Outcome, Pair, PairFinding, Severity, SideText, TextPair};

const FILLERS: u32 = 80;

fn function(path: &str, qualname: &str, line: u32, names: &[&str]) -> NamedFunction {
    let mut names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
    names.sort();
    let language = Language::from_path(path).expect("a known suffix");
    NamedFunction {
        id: format!("{path}::{qualname}"),
        path: path.to_owned(),
        language,
        qualname: qualname.to_owned(),
        line,
        name_line: line,
        protocol: false,
        test: false,
        names,
    }
}

// Functions whose names no other function has, so each name of the cases is rare.
fn fillers() -> Vec<NamedFunction> {
    (0..FILLERS)
        .map(|at| {
            let names = [format!("x{at}"), format!("y{at}"), format!("z{at}")];
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            function("src/fill.py", &format!("fill_{at}"), 10 + 3 * at, &names)
        })
        .collect()
}

fn tree(functions: Vec<NamedFunction>) -> NamedTree {
    let mut functions: Vec<NamedFunction> = fillers().into_iter().chain(functions).collect();
    functions.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
    NamedTree::Built {
        functions,
        unread: BTreeMap::new(),
        files: 3,
    }
}

// The new file src/new.py, whose functions `f` and `h` start on lines 1 and 4.
fn new_file() -> Vec<Pair> {
    let text = "def f():\n    return 1\n\ndef h():\n    return 2\n";
    let texts = vec![TextPair {
        path: "src/new.py".to_owned(),
        old: None,
        new: Some(text.to_owned()),
    }];
    Pair::measure_all(texts, |sides| sides.iter().map(SideText::measure).collect())
}

fn run(tree: &NamedTree, linked: &BTreeSet<Link>) -> Outcome {
    let pairs = new_file();
    let refs: Vec<&Pair> = pairs.iter().collect();
    let proposed = reuse_candidates(&refs, tree).expect("the check runs");
    proposed.outcome(linked)
}

// Each row as (location, other function, score, shared names).
fn rows(outcome: &Outcome) -> Vec<(String, String, String, Vec<String>)> {
    (outcome.findings.iter())
        .map(|row: &PairFinding| match &row.detail {
            Some(Detail::Reuse(share)) => (
                row.finding.location(),
                share.other.clone(),
                share.score.clone(),
                share.shared.clone(),
            ),
            other => panic!("not a reuse row: {other:?}"),
        })
        .collect()
}

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn an_added_function_that_shares_most_of_its_rare_names_is_a_row_with_its_words() {
    let tree = tree(vec![
        function("src/new.py", "f", 1, &["a", "b", "c", "d"]),
        function("src/old.py", "g", 1, &["a", "b", "c", "d", "e"]),
    ]);
    let outcome = run(&tree, &BTreeSet::new());
    let expected = (
        "src/new.py:1".to_owned(),
        "g".to_owned(),
        "0.785".to_owned(),
        names(&["a", "b", "c", "d"]),
    );
    assert_eq!(rows(&outcome), [expected]);
    let finding = &outcome.findings[0].finding;
    assert_eq!(finding.severity, Severity::Advisory);
    assert_eq!(
        finding.message,
        "`g` at src/old.py:1 calls and reads 4 of the same names as `f`: `a`, `b`, `c`, `d`. Read \
         `g` before you keep a second copy."
    );
    assert_eq!(finding.evidence, "score 0.785");
}

#[test]
fn a_filtered_candidate_gives_no_row() {
    let base = function("src/old.py", "g", 1, &["a", "b", "c", "d"]);
    let cases: [(&str, NamedFunction); 4] = [
        (
            "protocol",
            NamedFunction {
                protocol: true,
                ..base.clone()
            },
        ),
        (
            "test",
            NamedFunction {
                test: true,
                ..base.clone()
            },
        ),
        ("too few names", function("src/old.py", "g", 1, &["a", "b"])),
        (
            "another language",
            function("src/old.rs", "g", 1, &["a", "b", "c", "d"]),
        ),
    ];
    for (case, other) in cases {
        let tree = tree(vec![
            function("src/new.py", "f", 1, &["a", "b", "c", "d"]),
            other,
        ]);
        assert_eq!(rows(&run(&tree, &BTreeSet::new())), [], "{case}");
    }
}

#[test]
fn a_name_that_many_functions_share_does_not_count_and_a_linked_row_is_left_out() {
    let common: Vec<NamedFunction> = (0..5)
        .map(|at| {
            function(
                "src/many.py",
                &format!("m{at}"),
                1 + at,
                &["a", "b", "c", "q"],
            )
        })
        .collect();
    let mut functions = vec![function("src/new.py", "f", 1, &["a", "b", "c", "d"])];
    functions.extend(common);
    let outcome = run(&tree(functions), &BTreeSet::new());
    assert_eq!(
        rows(&outcome),
        [],
        "a, b and c are in 6 of 86 functions, past 5%"
    );

    let pair = tree(vec![
        function("src/new.py", "f", 1, &["a", "b", "c", "d"]),
        function("src/old.py", "g", 1, &["a", "b", "c", "d", "e"]),
    ]);
    let NamedTree::Built { functions, .. } = &pair else {
        panic!("a built tree");
    };
    let at = |qualname: &str| (functions.iter()).position(|function| function.qualname == qualname);
    let link = (at("f").expect("f"), at("g").expect("g"));
    assert_eq!(rows(&run(&pair, &BTreeSet::from([link]))), []);
}

#[test]
fn an_added_function_gives_its_two_best_rows_and_two_added_twins_give_one() {
    let three = tree(vec![
        function("src/new.py", "f", 1, &["a", "b", "c", "d"]),
        function("src/one.py", "g1", 1, &["a", "b", "c", "d", "e"]),
        function("src/two.py", "g2", 1, &["a", "b", "c", "d", "e", "k"]),
        function("src/three.py", "g3", 1, &["a", "b", "c", "d"]),
    ]);
    let others: Vec<String> = (rows(&run(&three, &BTreeSet::new())).into_iter())
        .map(|(_, other, _, _)| other)
        .collect();
    // g3 and g1 score best; the report orders rows by place, then message.
    assert_eq!(others, ["g1", "g3"]);

    let twins = tree(vec![
        function("src/new.py", "f", 1, &["a", "b", "c", "d"]),
        function("src/new.py", "h", 4, &["a", "b", "c", "d"]),
    ]);
    let found: Vec<(String, String)> = (rows(&run(&twins, &BTreeSet::new())).into_iter())
        .map(|(at, other, _, _)| (at, other))
        .collect();
    assert_eq!(found, [("src/new.py:1".to_owned(), "h".to_owned())]);
}
