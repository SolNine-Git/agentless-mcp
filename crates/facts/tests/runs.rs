//! When each Python import statement runs: at load, when its function runs, or only for a type
//! checker.

use std::sync::Arc;

use facts::{Runs, extract};
use syntax::{Language, parse};

// Each import of `source` as (line, module, runs).
fn runs(language: Language, source: &str) -> Vec<(u32, String, Runs)> {
    let parsed = parse(language, Arc::from(source)).expect("the sample parses");
    extract(&parsed)
        .imports
        .into_iter()
        .map(|import| (import.line, import.module, import.runs))
        .collect()
}

fn row(line: u32, module: &str, runs: Runs) -> (u32, String, Runs) {
    (line, module.to_owned(), runs)
}

#[test]
fn an_import_in_a_function_or_a_method_runs_when_it_is_called() {
    let source = "import a\n\n\ndef f():\n    import b\n    from .c import d\n\n\n\
                  class K:\n    import e\n\n    def m(self):\n        if self:\n            \
                  from f import g\n\n\n\
                  def outer():\n    def inner():\n        import h\n    return inner\n\n\n\
                  @wrap\nasync def run():\n    import i\n";
    assert_eq!(
        runs(Language::Python, source),
        [
            row(1, "a", Runs::Load),
            row(5, "b", Runs::Call),
            row(6, "c", Runs::Call),
            row(10, "e", Runs::Load),
            row(14, "f", Runs::Call),
            row(19, "h", Runs::Call),
            row(25, "i", Runs::Call),
        ]
    );
}

#[test]
fn an_import_under_type_checking_is_type_only_and_its_else_branch_runs_at_load() {
    let source = "from typing import TYPE_CHECKING\nimport typing\n\n\
                  if TYPE_CHECKING:\n    import a\n    try:\n        import b\n    except ImportError:\n        pass\n\
                  else:\n    import c\n\n\
                  if typing.TYPE_CHECKING:\n    import d\nelif x:\n    import e\n\n\
                  if not TYPE_CHECKING:\n    import f\n\n\
                  if TYPE_CHECKING: import g\n\n\n\
                  def h():\n    if TYPE_CHECKING:\n        import i\n";
    assert_eq!(
        runs(Language::Python, source),
        [
            row(1, "typing", Runs::Load),
            row(2, "typing", Runs::Load),
            row(5, "a", Runs::TypeOnly),
            row(7, "b", Runs::TypeOnly),
            row(11, "c", Runs::Load),
            row(14, "d", Runs::TypeOnly),
            row(16, "e", Runs::Load),
            row(19, "f", Runs::Load),
            row(21, "g", Runs::TypeOnly),
            row(26, "i", Runs::Call),
        ]
    );
}

#[test]
fn a_script_block_and_a_guarded_import_run_at_load() {
    let source = "try:\n    import a\nexcept ImportError:\n    a = None\n\n\
                  if __name__ == \"__main__\":\n    import b\n\nwith open(p) as h:\n    import c\n";
    assert_eq!(
        runs(Language::Python, source),
        [
            row(2, "a", Runs::Load),
            row(7, "b", Runs::Load),
            row(10, "c", Runs::Load),
        ]
    );
}

#[test]
fn only_the_python_walker_tells_when_an_import_runs() {
    let rust = "use a::b;\n\nfn f() {\n    use c::d;\n}\n";
    let found: Vec<Runs> = runs(Language::Rust, rust)
        .into_iter()
        .map(|(_, _, runs)| runs)
        .collect();
    assert_eq!(found, [Runs::Load, Runs::Load]);
}
