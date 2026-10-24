//! Definitions nested deeper than a small stack holds frames for: each walker whose depth the
//! source sets keeps its own stack, so the extraction finishes on a 256 KiB thread. Where the
//! reference runs out of recursion, the recorded deviation applies: the facts are whole.

use std::fmt::Write as _;
use std::sync::Arc;

use facts::{Symbol, SymbolKind, extract};
use serde_json::Value;
use syntax::{Language, parse};
use text::testing::cases;

const STACK_BYTES: usize = 256 * 1024;

#[test]
fn nested_python_functions_extract_on_a_small_stack() {
    check_shape("defs");
}

#[test]
fn nested_decorated_python_classes_extract_on_a_small_stack() {
    check_shape("classes");
}

#[test]
fn nested_c_function_declarators_extract_on_a_small_stack() {
    check_shape("c_declarators");
}

// Each recorded depth of one shape: the reference's symbols, or the whole facts where the
// reference ran out of recursion.
fn check_shape(wanted: &str) {
    let all: Vec<(String, usize, Value)> = cases(env!("CARGO_MANIFEST_DIR"), "views", "depth");
    let chosen: Vec<(String, usize, Value)> = all
        .into_iter()
        .filter(|(shape, _, _)| shape == wanted)
        .collect();
    assert_eq!(chosen.len(), 2, "{wanted}");
    for (shape, depth, outcome) in chosen {
        let symbols = extract_on_small_stack(&shape, depth);
        let found: Vec<(String, String)> = symbols
            .iter()
            .map(|symbol| (symbol.name.clone(), symbol.kind.as_str().to_owned()))
            .collect();
        match outcome[0].as_str() {
            Some("symbols") => {
                let expected: Vec<(String, String)> =
                    serde_json::from_value(outcome[1].clone()).expect("name and kind pairs");
                assert_eq!(found, expected, "{shape} at depth {depth}");
            }
            _ => assert_whole(&shape, depth, &symbols),
        }
    }
}

// The deviation: where the reference raises at its recursion limit, each definition is a symbol.
fn assert_whole(shape: &str, depth: usize, symbols: &[Symbol]) {
    if shape == "classes" {
        assert_eq!(symbols.len(), depth + 1, "{shape} at depth {depth}");
        assert!(
            symbols
                .iter()
                .all(|symbol| symbol.kind == SymbolKind::Class)
        );
        assert_eq!(symbols[depth].name, format!("C{depth}"));
    } else {
        let found: Vec<(&str, SymbolKind)> = symbols
            .iter()
            .map(|symbol| (symbol.name.as_str(), symbol.kind))
            .collect();
        assert_eq!(
            found,
            [("f", SymbolKind::Function)],
            "{shape} at depth {depth}"
        );
    }
}

// Parse on this thread, then extract on a thread with a small stack.
fn extract_on_small_stack(shape: &str, depth: usize) -> Vec<Symbol> {
    let (language, source) = deep_source(shape, depth);
    let parsed = parse(language, Arc::from(source)).expect("the source parses");
    std::thread::Builder::new()
        .stack_size(STACK_BYTES)
        .spawn(move || extract(&parsed).symbols)
        .expect("the thread starts")
        .join()
        .expect("the extraction finishes")
}

// The same sources that the fixture generator builds for each shape.
fn deep_source(shape: &str, depth: usize) -> (Language, String) {
    let mut source = String::new();
    match shape {
        "defs" => {
            for level in 0..depth {
                writeln!(source, "{}def f{level}():", " ".repeat(level)).expect("a String");
            }
            writeln!(source, "{}pass", " ".repeat(depth)).expect("a String");
            (Language::Python, source)
        }
        "classes" => {
            source.push_str("class C0:\n");
            for level in 1..=depth {
                let indent = " ".repeat(level);
                writeln!(source, "{indent}@d\n{indent}class C{level}:").expect("a String");
            }
            writeln!(source, "{}pass", " ".repeat(depth + 1)).expect("a String");
            (Language::Python, source)
        }
        _ => {
            let source = format!(
                "int {}f(void){};\n",
                "(*".repeat(depth),
                ")(int)".repeat(depth)
            );
            (Language::C, source)
        }
    }
}
