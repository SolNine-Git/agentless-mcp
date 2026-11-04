//! The protocol and test marks of Rust and Python declarations.

use std::sync::Arc;

use facts::extract;
use syntax::{Language, parse};

// Each function or method of `source` by qualified name, with its (protocol, test) marks.
fn marks(language: Language, source: &str) -> Vec<(String, bool, bool)> {
    let parsed = parse(language, Arc::from(source)).expect("the sample parses");
    extract(&parsed)
        .symbols
        .iter()
        .map(|symbol| {
            let marks = &symbol.marks;
            (symbol.qualname(), marks.protocol, marks.test)
        })
        .collect()
}

#[test]
fn a_trait_impl_method_is_a_protocol_and_an_inherent_one_is_not() {
    let source = "struct P;\nimpl Clone for P {\n    fn clone(&self) -> P { P }\n}\n\
                  impl P {\n    fn new() -> P { P }\n}\n";
    let found = marks(Language::Rust, source);
    assert_eq!(
        found,
        [
            ("P".to_owned(), false, false),
            ("P.clone".to_owned(), true, false),
            ("P.new".to_owned(), false, false),
        ]
    );
}

#[test]
fn a_test_attribute_or_a_cfg_test_around_a_function_marks_it_as_a_test() {
    let source = "#[tokio::test]\nasync fn runs() {}\n\
                  #[test]\n// a comment between\n#[ignore]\nfn skipped() {}\n\
                  #[cfg(test)]\nfn helper() {}\n\
                  #[cfg(test)]\nimpl Q {\n    fn build() {}\n}\n\
                  #[cfg(test)]\nmod tests {\n    mod deep {\n        fn inner() {}\n    }\n}\n\
                  #[cfg(feature = \"x\")]\nfn gated() {}\n\
                  #[testing]\nfn named() {}\n";
    let found = marks(Language::Rust, source);
    assert_eq!(
        found,
        [
            ("runs".to_owned(), false, true),
            ("skipped".to_owned(), false, true),
            ("helper".to_owned(), false, true),
            ("Q.build".to_owned(), false, true),
            ("inner".to_owned(), false, true),
            ("gated".to_owned(), false, false),
            ("named".to_owned(), false, false),
        ]
    );
}

#[test]
fn a_python_dunder_is_a_protocol_and_no_python_function_is_a_test() {
    let source = "class Box:\n    def __init__(self):\n        pass\n\n    def _private(self):\n        pass\n\n\
                  def __getattr__(name):\n    pass\n\n\ndef test_box():\n    pass\n\n\ndef ____():\n    pass\n";
    let found = marks(Language::Python, source);
    assert_eq!(
        found,
        [
            ("Box".to_owned(), false, false),
            ("Box.__init__".to_owned(), true, false),
            ("Box._private".to_owned(), false, false),
            ("__getattr__".to_owned(), true, false),
            ("test_box".to_owned(), false, false),
            ("____".to_owned(), false, false),
        ]
    );
}
