use std::collections::{BTreeMap, BTreeSet};

use metrics::Unmeasurable;

use super::{
    BudgetSpent, DodgeTree, ImportTarget, LoadChains, LoadedModule, LocalImport, LocalImports,
    Tree, Unparsed, cycle_dodge,
};
use crate::{ChainHop, Detail, Outcome, Pair, SideError, SideText, TextPair};

// The chains of each tree as given, and each search the check asked for.
#[derive(Default)]
struct Given {
    chains: BTreeMap<(Tree, String, String), Vec<ChainHop>>,
    asked: Vec<(Tree, String, String)>,
    spent: Option<u64>,
}

impl Given {
    // A chain of `tree` that runs through `files` in order, each hop on line 1.
    fn chain(mut self, tree: Tree, files: &[&str]) -> Given {
        let hops = files.windows(2).map(|pair| ChainHop {
            path: pair[0].to_owned(),
            line: 1,
            target: pair[1].to_owned(),
        });
        let (from, to) = (files[0].to_owned(), files[files.len() - 1].to_owned());
        self.chains.insert((tree, from, to), hops.collect());
        self
    }
}

impl LoadChains for Given {
    fn chain(
        &mut self,
        tree: Tree,
        from: &str,
        to: &str,
    ) -> Result<Option<Vec<ChainHop>>, BudgetSpent> {
        if let Some(seconds) = self.spent {
            return Err(BudgetSpent { seconds });
        }
        let key = (tree, from.to_owned(), to.to_owned());
        self.asked.push(key.clone());
        Ok(self.chains.get(&key).cloned())
    }
}

// An import of `module` in `function` of `path` at `line`, of the file `module`.py.
fn local(path: &str, line: u32, function: &str, module: &str) -> LocalImport {
    LocalImport {
        path: path.to_owned(),
        line,
        function: Some(function.to_owned()),
        module: module.to_owned(),
        level: 0,
        names: Vec::new(),
        targets: vec![ImportTarget {
            module: module.to_owned(),
            path: format!("{module}.py"),
        }],
    }
}

fn run(pairs: &[Pair], imports: LocalImports, given: &mut Given) -> Outcome {
    let refs: Vec<&Pair> = pairs.iter().collect();
    let tree = DodgeTree::Built {
        imports,
        chains: given,
    };
    cycle_dodge(&refs, tree).expect("the check runs")
}

// Each row as (location, via, chain files).
fn rows(outcome: &Outcome) -> Vec<(String, &'static str, Vec<String>)> {
    let row = |found: &crate::PairFinding| match &found.detail {
        Some(Detail::Dodge(dodge)) => (
            found.finding.location(),
            dodge.via.as_str(),
            dodge.chain.iter().map(|hop| hop.path.clone()).collect(),
        ),
        other => panic!("not a dodge row: {other:?}"),
    };
    outcome.findings.iter().map(row).collect()
}

fn row(location: &str, via: &'static str, chain: &[&str]) -> (String, &'static str, Vec<String>) {
    let chain = chain.iter().map(|path| (*path).to_owned()).collect();
    (location.to_owned(), via, chain)
}

#[test]
fn a_new_import_whose_file_the_target_imports_back_at_load_is_a_row_with_its_texts() {
    let imports = LocalImports {
        after: vec![local("a.py", 7, "Box.load", "b")],
        ..LocalImports::default()
    };
    let mut given = Given::default().chain(Tree::After, &["b.py", "c.py", "a.py"]);
    let outcome = run(&[], imports, &mut given);
    let found = &outcome.findings[0].finding;
    assert_eq!(
        (
            found.location(),
            found.message.as_str(),
            found.evidence.as_str()
        ),
        (
            "a.py:7".to_owned(),
            "The import of `b` in `Box.load` runs only when the function runs. At module level it \
             closes this import cycle: a.py -> b.py -> c.py -> a.py. Break the cycle: move the \
             shared code to a module that both files import, or invert the dependency.",
            "a.py:7 imports b.py; b.py imports a.py back through b.py:1 -> c.py:1",
        )
    );
    assert_eq!(rows(&outcome), [row("a.py:7", "new", &["b.py", "c.py"])]);
}

#[test]
fn an_import_with_no_chain_back_at_load_is_no_row() {
    let imports = LocalImports {
        after: vec![local("a.py", 7, "load", "b")],
        ..LocalImports::default()
    };
    let mut given = Given::default().chain(Tree::Before, &["b.py", "a.py"]);
    assert_eq!(rows(&run(&[], imports, &mut given)), []);
}

#[test]
fn an_import_that_the_file_held_at_load_before_the_change_moved_into_the_function() {
    let moved = LoadedModule {
        path: "a.py".to_owned(),
        module: "b".to_owned(),
        level: 0,
    };
    let imports = LocalImports {
        after: vec![local("a.py", 7, "load", "b")],
        before_loads: BTreeSet::from([moved]),
        loads_changed: true,
        ..LocalImports::default()
    };
    let mut given = Given::default().chain(Tree::After, &["b.py", "a.py"]);
    assert_eq!(
        rows(&run(&[], imports, &mut given)),
        [row("a.py:7", "moved", &["b.py"])]
    );
}

// The statement pairs by its identity, not its line: an edit above it moves it.
#[test]
fn an_unchanged_import_with_unchanged_imports_at_load_is_never_searched() {
    let imports = LocalImports {
        after: vec![local("a.py", 9, "load", "b")],
        before: vec![local("a.py", 7, "load", "b")],
        ..LocalImports::default()
    };
    let mut given = Given::default()
        .chain(Tree::After, &["b.py", "a.py"])
        .chain(Tree::Before, &["b.py", "a.py"]);
    assert_eq!(rows(&run(&[], imports, &mut given)), []);
    assert_eq!(given.asked, []);
}

#[test]
fn an_unchanged_import_is_a_row_only_when_the_change_made_its_chain() {
    let imports = |loads_changed| LocalImports {
        after: vec![local("a.py", 9, "load", "b")],
        before: vec![local("a.py", 7, "load", "b")],
        loads_changed,
        ..LocalImports::default()
    };
    let mut made = Given::default().chain(Tree::After, &["b.py", "a.py"]);
    assert_eq!(
        rows(&run(&[], imports(true), &mut made)),
        [row("a.py:9", "return_path", &["b.py"])]
    );
    let mut held = Given::default()
        .chain(Tree::After, &["b.py", "a.py"])
        .chain(Tree::Before, &["b.py", "c.py", "a.py"]);
    assert_eq!(rows(&run(&[], imports(true), &mut held)), []);
}

#[test]
fn the_first_target_with_a_chain_names_the_row_and_rows_come_in_path_and_line_order() {
    let mut two = local("b.py", 3, "B.go", "pkg");
    two.targets.push(ImportTarget {
        module: "pkg.mod".to_owned(),
        path: "pkg/mod.py".to_owned(),
    });
    two.function = None;
    let imports = LocalImports {
        after: vec![two, local("a.py", 5, "go", "c")],
        ..LocalImports::default()
    };
    let mut given = Given::default()
        .chain(Tree::After, &["pkg/mod.py", "b.py"])
        .chain(Tree::After, &["c.py", "a.py"]);
    let outcome = run(&[], imports, &mut given);
    assert_eq!(
        rows(&outcome),
        [
            row("a.py:5", "new", &["c.py"]),
            row("b.py:3", "new", &["pkg/mod.py"])
        ]
    );
    let message = &outcome.findings[1].finding.message;
    assert!(
        message.starts_with("The import of `pkg.mod` in `<module>` runs"),
        "{message}"
    );
}

#[test]
fn a_search_past_the_budget_gives_one_stopped_row_and_no_finding() {
    let stopped = "not checked: cycle_dodge stopped at its time budget of 10 s before it read the \
                   imports of every file in the repository, so it compared no import.";
    let imports = LocalImports {
        after: vec![local("a.py", 7, "load", "b")],
        ..LocalImports::default()
    };
    let mut given = Given {
        spent: Some(10),
        ..Given::default()
    };
    let outcome = run(&[], imports, &mut given);
    let messages: Vec<&str> = (outcome.findings.iter())
        .map(|row| row.finding.message.as_str())
        .collect();
    assert_eq!(messages, [stopped]);
    let scan = cycle_dodge(&[], DodgeTree::Stopped { seconds: 10 }).expect("the check runs");
    assert_eq!(scan, outcome);
}

// The facts decide: a.py's old side holds a syntax error, and c.py's new side only fails the
// tokenizer, so c.py is compared.
#[test]
fn a_side_whose_facts_hold_a_syntax_error_has_a_row_and_no_compared_import() {
    let tokenized = "LABEL = \"left\u{2028}right\"\n\n\ndef g():\n    import b\n";
    let texts = vec![
        TextPair {
            path: "a.py".to_owned(),
            old: Some("def f():\n    return f f\n".to_owned()),
            new: Some("def f():\n    import b\n".to_owned()),
        },
        TextPair {
            path: "c.py".to_owned(),
            old: None,
            new: Some(tokenized.to_owned()),
        },
    ];
    let pairs = Pair::measure_all(texts, |sides| sides.iter().map(SideText::measure).collect());
    let measured = pairs[1].measured_blocks(tokenized);
    assert!(
        matches!(
            measured,
            Some(Err(SideError::Unmeasurable(Unmeasurable::Tokenizer(_))))
        ),
        "{measured:?}"
    );
    let unparsed = Unparsed {
        path: "a.py".to_owned(),
        tree: Tree::Before,
        line: 2,
    };
    let imports = LocalImports {
        after: vec![local("a.py", 2, "f", "b"), local("c.py", 5, "g", "b")],
        unparsed: vec![unparsed],
        ..LocalImports::default()
    };
    let mut given = Given::default()
        .chain(Tree::After, &["b.py", "a.py"])
        .chain(Tree::After, &["b.py", "c.py"]);
    let outcome = run(&pairs, imports, &mut given);
    let found: Vec<(String, &str)> = (outcome.findings.iter())
        .map(|row| (row.finding.location(), row.finding.severity.as_str()))
        .collect();
    let unread = ("a.py".to_owned(), "not_checked");
    assert_eq!(found, [unread, ("c.py:5".to_owned(), "advisory")]);
    assert_eq!(
        outcome.findings[0].finding.message,
        "not checked: cycle_dodge did not compare a.py, because its text before the change does \
         not parse. a.py does not parse: the first syntax error is at line 2. Imports are compared \
         only in a file that parses. Fix the syntax error, then run the check again."
    );
    let asked = (Tree::After, "b.py".to_owned(), "c.py".to_owned());
    assert_eq!(given.asked, [asked]);
}
