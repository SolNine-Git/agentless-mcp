use std::fmt::Write as _;

use super::{Callee, CalleeTree, TreeFunction, callee_overlap};
use crate::{Detail, Outcome, Pair, PairFinding, SideText, TextPair};

// A function of the tree whose callees are helpers of lib.py, each bound in the same file.
fn function(path: &str, name: &str, line: u32, callees: &[&str], spelled: &[&str]) -> TreeFunction {
    let mut spelled: Vec<String> = spelled.iter().map(|name| (*name).to_owned()).collect();
    spelled.sort();
    spelled.dedup();
    TreeFunction {
        path: path.to_owned(),
        name: name.to_owned(),
        qualname: name.to_owned(),
        line,
        name_line: line,
        callees: callees
            .iter()
            .map(|callee| Callee {
                id: format!("py:lib.py::{callee}"),
                name: (*callee).to_owned(),
                qualname: (*callee).to_owned(),
                tier: "imported",
            })
            .collect(),
        spelled,
    }
}

// A body that calls each of `callees`, as a Python function named `name`.
fn body(name: &str, callees: &[&str]) -> String {
    callees
        .iter()
        .fold(format!("def {name}():\n"), |mut text, callee| {
            writeln!(text, "    {callee}()").expect("a String takes every write");
            text
        })
}

fn pairs(rows: &[(&str, Option<String>, Option<String>)]) -> Vec<Pair> {
    let texts = rows
        .iter()
        .map(|(path, old, new)| TextPair {
            path: (*path).to_owned(),
            old: old.clone(),
            new: new.clone(),
        })
        .collect();
    Pair::measure_all(texts, |sides| sides.iter().map(SideText::measure).collect())
}

fn run(pairs: &[Pair], functions: Vec<TreeFunction>) -> Outcome {
    let refs: Vec<&Pair> = pairs.iter().collect();
    callee_overlap(&refs, &CalleeTree::Built(functions)).expect("the check runs")
}

// Each row as (location, other function, shared count, family).
fn rows(outcome: &Outcome) -> Vec<(String, String, usize, usize)> {
    outcome
        .findings
        .iter()
        .map(|row: &PairFinding| match &row.detail {
            Some(Detail::Overlap(share)) => (
                row.finding.location(),
                share.other.clone(),
                share.shared.len(),
                share.family,
            ),
            other => panic!("not an overlap row: {other:?}"),
        })
        .collect()
}

const FOUR: [&str; 4] = ["a", "b", "c", "d"];

#[test]
fn a_new_function_that_calls_four_of_five_callees_is_a_row_and_three_of_four_is_not() {
    let pairs = pairs(&[("src/new.py", None, Some(body("f", &FOUR)))]);
    let functions = vec![
        function("src/old.py", "g", 1, &["a", "b", "c", "d", "e"], &["g"]),
        function("src/old.py", "k", 9, &["a", "b", "c", "x"], &["k"]),
        function("src/new.py", "f", 1, &FOUR, &["f", "a", "b", "c", "d"]),
    ];
    let found = rows(&run(&pairs, functions));
    assert_eq!(found, [("src/new.py:1".to_owned(), "g".to_owned(), 4, 0)]);
}

#[test]
fn an_existing_function_with_two_callees_or_a_name_the_new_one_spells_gives_no_row() {
    let new = body("f", &["a", "b", "g"]);
    let pairs = pairs(&[("src/new.py", None, Some(new))]);
    let functions = vec![
        function(
            "src/old.py",
            "g",
            1,
            &["a", "b", "c"],
            &["g", "a", "b", "c"],
        ),
        function("src/old.py", "two", 9, &["a", "b"], &["two", "a", "b"]),
        function(
            "src/new.py",
            "f",
            1,
            &["a", "b", "c"],
            &["f", "a", "b", "g"],
        ),
    ];
    assert_eq!(rows(&run(&pairs, functions)), []);
}

// The kin spell no name of the new function, and it spells each of theirs, so only g is a
// candidate of the new function.
#[test]
fn five_other_functions_that_meet_the_rule_make_a_family_and_four_do_not() {
    let pairs = pairs(&[("src/new.py", None, Some(body("f", &["a", "b", "c"])))]);
    let kin = ["s1", "s2", "s3", "s4", "s5"];
    let mut spelled = vec!["f"];
    spelled.extend(kin);
    let mut functions = vec![
        function("src/old.py", "g", 1, &["a", "b", "c"], &["g"]),
        function("src/new.py", "f", 1, &["a", "b", "c"], &spelled),
    ];
    for (at, name) in kin.into_iter().enumerate() {
        let line = 10 + u32::try_from(at).expect("a small index");
        functions.push(function(
            "src/kin.py",
            name,
            line,
            &["a", "b", "c"],
            &[name],
        ));
    }
    let five = rows(&run(&pairs, functions.clone()));
    assert_eq!(five, []);
    functions.pop();
    let four = rows(&run(&pairs, functions));
    assert_eq!(four, [("src/new.py:1".to_owned(), "g".to_owned(), 3, 4)]);
}

#[test]
fn a_unit_gives_at_most_three_rows_ranked_by_shared_count_then_set_size() {
    let callees = ["a", "b", "c", "d", "e"];
    let new = body("f", &callees);
    let pairs = pairs(&[("src/new.py", None, Some(new))]);
    let functions = vec![
        function("src/new.py", "f", 1, &callees, &["f"]),
        function("src/o.py", "three", 1, &["a", "b", "c"], &["three"]),
        function("src/o.py", "four", 5, &["a", "b", "c", "d"], &["four"]),
        function("src/o.py", "five", 9, &callees, &["five"]),
        function("src/o.py", "other", 15, &["a", "b", "c"], &["other"]),
    ];
    let found = rows(&run(&pairs, functions));
    let others: Vec<&str> = found.iter().map(|row| row.1.as_str()).collect();
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(
        others.contains(&"five") && others.contains(&"four"),
        "{others:?}"
    );
    assert!(
        others.contains(&"three") != others.contains(&"other"),
        "{others:?}"
    );
    assert!(
        others.contains(&"three"),
        "the path and line break the tie: {others:?}"
    );
}

#[test]
fn a_rewrite_is_a_row_only_when_its_old_body_spelled_less_than_four_fifths_of_the_callees() {
    let old_far = body("f", &["x"]);
    let old_near = body("f", &["a", "b", "c", "d", "y"]);
    let new = body("f", &FOUR);
    let functions = vec![
        function("src/old.py", "g", 1, &FOUR, &["g"]),
        function("src/f.py", "f", 1, &FOUR, &["f", "a", "b", "c", "d"]),
    ];
    let far = pairs(&[("src/f.py", Some(old_far), Some(new.clone()))]);
    assert_eq!(rows(&run(&far, functions.clone())).len(), 1);
    let near = pairs(&[("src/f.py", Some(old_near), Some(new))]);
    assert_eq!(rows(&run(&near, functions)), []);
}

#[test]
fn two_new_functions_that_meet_the_rule_both_ways_give_one_row() {
    let new = format!(
        "{}\n\n{}",
        body("f", &["a", "b", "c"]),
        body("g", &["a", "b", "c"])
    );
    let pairs = pairs(&[("src/new.py", None, Some(new))]);
    let functions = vec![
        function("src/new.py", "f", 1, &["a", "b", "c"], &["f"]),
        function("src/new.py", "g", 7, &["a", "b", "c"], &["g"]),
    ];
    let found = rows(&run(&pairs, functions));
    assert_eq!(found, [("src/new.py:1".to_owned(), "g".to_owned(), 3, 0)]);
}

#[test]
fn a_moved_function_is_not_new_and_a_stopped_build_gives_one_row() {
    let text = body("f", &FOUR);
    let pairs = pairs(&[
        ("src/a.py", Some(text.clone()), None),
        ("src/b.py", None, Some(text)),
    ]);
    let functions = vec![
        function("src/old.py", "g", 1, &FOUR, &["g"]),
        function("src/b.py", "f", 1, &FOUR, &["f"]),
    ];
    assert_eq!(rows(&run(&pairs, functions)), []);
    let refs: Vec<&Pair> = pairs.iter().collect();
    let stopped = callee_overlap(&refs, &CalleeTree::Stopped { seconds: 10 }).expect("a row");
    let messages: Vec<&str> = stopped
        .findings
        .iter()
        .map(|row| row.finding.message.as_str())
        .collect();
    assert_eq!(
        messages,
        [
            "not checked: callee_overlap stopped at its time budget of 10 s before it read the calls \
          of every function in the repository, so it compared no function."
        ]
    );
}

#[test]
fn a_side_with_a_syntax_error_or_a_tokenizer_error_gives_a_reason_that_names_functions() {
    let (syntax, tokenizer) = ("def f(:\n    pass\n", "LABEL = \"left\u{2028}right\"\n");
    let pairs = pairs(&[
        ("a.py", None, Some(syntax.to_owned())),
        ("b.py", None, Some(tokenizer.to_owned())),
    ]);
    let outcome = run(&pairs, Vec::new());
    let messages: Vec<&str> = (outcome.findings.iter())
        .map(|row| row.finding.message.as_str())
        .collect();
    assert_eq!(
        messages,
        [
            "not checked: callee_overlap did not compare a.py, because its text after the change \
             cannot be measured. a.py does not parse: the first syntax error is at line 1. \
             Functions are compared only in a file that parses. Fix the syntax error, then run \
             the check again.",
            "not checked: callee_overlap did not compare b.py, because its text after the change \
             cannot be measured. b.py: Python's tokenizer rejects this file (TokenError: \
             unterminated string literal (detected at line 1)). Functions are compared only in a \
             file the tokenizer accepts. Fix the line the error names, then run the check again."
        ]
    );
}

#[test]
fn the_row_names_both_functions_and_its_detail_carries_both_callee_sets_with_tiers() {
    let new = body("f", &FOUR);
    let pairs = pairs(&[("src/new.py", None, Some(new))]);
    let functions = vec![
        function("src/old.py", "g", 3, &["a", "b", "c", "d", "e"], &["g"]),
        function("src/new.py", "f", 1, &FOUR, &["f"]),
    ];
    let outcome = run(&pairs, functions);
    let row = &outcome.findings[0];
    assert_eq!(
        row.finding.message,
        "`f` calls 4 of the 5 functions that `g` calls, but it does not call `g`. If the two do \
         one job, call `g` and remove the copy. If they only use the same helpers, ignore this row."
    );
    assert_eq!(
        row.finding.evidence,
        "`g` at src/old.py:3; shared: a, b, c, d"
    );
    let json = serde_json::to_value(row).expect("a row serializes");
    assert_eq!(json["severity"], "advisory");
    assert_eq!(json["detail"]["match"], "new");
    assert_eq!(json["detail"]["other_path"], "src/old.py");
    assert_eq!(json["detail"]["other_line"], 3);
    assert_eq!(
        json["detail"]["shared"][0],
        serde_json::json!({"callee": "py:lib.py::a", "tier": "imported", "other_tier": "imported"})
    );
    assert_eq!(
        json["detail"]["other_callees"].as_array().map(Vec::len),
        Some(5)
    );
}
