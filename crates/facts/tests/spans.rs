//! A definition's span starts at the first decorator, attribute or overload signature that
//! belongs to it, its name line stays on its name, a slice heads with that name line, and a
//! TOML table ends at its last key.

use std::sync::Arc;

use facts::{extract, slice_lines};
use syntax::{Language, parse};

// Each symbol as (qualified name, line, end line, name line).
fn spans(language: Language, source: &str) -> Vec<(String, u32, u32, u32)> {
    let parsed = parse(language, Arc::from(source)).expect("the source parses");
    let symbols = extract(&parsed).symbols;
    symbols
        .iter()
        .map(|symbol| {
            let name = if symbol.parent.is_empty() {
                symbol.name.clone()
            } else {
                format!("{}.{}", symbol.parent, symbol.name)
            };
            (name, symbol.line, symbol.end_line, symbol.name_line)
        })
        .collect()
}

fn rows(expected: &[(&str, u32, u32, u32)]) -> Vec<(String, u32, u32, u32)> {
    let rows = expected.iter();
    rows.map(|(name, line, end, name_line)| ((*name).to_owned(), *line, *end, *name_line))
        .collect()
}

#[test]
fn a_python_definition_starts_at_its_first_decorator() {
    let source = "import functools\n\n\n@functools.cache\n@other(\n    arg=1,\n)\ndef top():\n    \
                  return 1\n\n\n@dataclass\nclass Point:\n    x: int\n\n    @property\n    def \
                  norm(self):\n        return 0\n\n    @staticmethod\n    # a comment between\n    \
                  def make():\n        def inner():\n            pass\n\n        @wraps(inner)\n  \
                  \x20     def wrapped():\n            pass\n\n        return wrapped\n";
    let expected = [
        ("top", 4, 9, 8),
        ("Point", 12, 30, 13),
        ("Point.norm", 16, 18, 17),
        ("Point.make", 20, 30, 22),
        ("Point.make.inner", 23, 24, 23),
        ("Point.make.wrapped", 26, 28, 27),
    ];
    assert_eq!(spans(Language::Python, source), rows(&expected));
}

#[test]
fn a_rust_item_starts_at_its_first_outer_attribute() {
    let source = "#![allow(dead_code)]\n\n#[derive(Debug, Clone)]\n#[serde(rename_all = \
                  \"snake_case\")]\npub struct Point {\n    x: i32,\n}\n\n/// Doc comment.\n\
                  #[derive(Debug)]\n/// More doc.\nenum Shape {\n    Circle,\n}\n\n#[cfg(test)]\nmod \
                  tests {\n    #[test]\n    fn works() {\n        assert!(true);\n    }\n}\n\nimpl \
                  Point {\n    #[must_use]\n    #[inline]\n    pub fn new() -> Point {\n        \
                  Point { x: 0 }\n    }\n}\n\n#[inline]\nfn free() {}\n";
    let expected = [
        ("Point", 3, 7, 5),
        ("Shape", 10, 14, 12),
        ("works", 18, 21, 19),
        ("Point.new", 25, 29, 27),
        ("free", 32, 33, 33),
    ];
    assert_eq!(spans(Language::Rust, source), rows(&expected));
}

#[test]
fn a_typescript_definition_starts_at_its_decorators_and_overload_signatures() {
    let source = "@Component({\n    selector: \"x\",\n})\nexport class Widget {\n    @Input() \
                  name: string;\n\n    @HostListener(\"click\")\n    onClick(): void {\n        \
                  return;\n    }\n\n    public static get(): number;\n    public static get(key: \
                  string): string;\n    public static get(key?: string): number | string {\n        \
                  return 0;\n    }\n}\n\n@Injectable()\nclass Service {}\n\nexport function over(a: \
                  string): string;\nexport function over(a: number): number;\nexport function \
                  over(a: any): any {\n    return a;\n}\n\nfunction plain(a: string): \
                  string;\nfunction plain(a: any): any {\n    return a;\n}\n";
    let expected = [
        ("Widget", 1, 17, 4),
        ("Widget.onClick", 7, 10, 8),
        ("Widget.get", 12, 16, 14),
        ("Service", 19, 20, 20),
        ("over", 22, 26, 24),
        ("plain", 28, 31, 29),
    ];
    assert_eq!(spans(Language::Typescript, source), rows(&expected));
    assert_eq!(spans(Language::Tsx, source), rows(&expected));
}

#[test]
fn a_java_annotation_inside_the_node_keeps_the_span_and_moves_the_name_line() {
    let source = "package x;\n\n@Entity\n@Table(name = \"t\")\npublic class Deco {\n    \
                  @Override\n    @Deprecated\n    public String toString() {\n        return \
                  \"\";\n    }\n\n    @Inject\n    private int field;\n}\n";
    let expected = [("Deco", 3, 14, 5), ("Deco.toString", 6, 10, 8)];
    assert_eq!(spans(Language::Java, source), rows(&expected));
}

#[test]
fn a_toml_table_ends_at_its_last_key_or_at_its_header() {
    let source = "[a]\nx = 1\ny = [\n  2,\n]\n\n[b]\n\n[c]\nz = 3\n";
    let tables: Vec<(String, u32, u32, u32)> = spans(Language::Toml, source)
        .into_iter()
        .filter(|(name, ..)| ["a", "b", "c"].contains(&name.as_str()))
        .collect();
    let expected = [("a", 1, 5, 1), ("b", 7, 7, 7), ("c", 9, 10, 9)];
    assert_eq!(tables, rows(&expected));
}

fn sliced(language: Language, source: &str, line: u32) -> String {
    let parsed = parse(language, Arc::from(source)).expect("the source parses");
    let symbols = extract(&parsed).symbols;
    slice_lines(source, &[(line, line)], &symbols).expect("the line is in the file")
}

#[test]
fn a_slice_heads_with_the_name_line_not_the_decorator_or_annotation_line() {
    let java = "package x;\n\n@Entity\n@Table(name = \"t\")\npublic class Deco {\n    \
                @Override\n    @Deprecated\n    public String toString() {\n        return \
                \"\";\n    }\n}\n";
    let expected = "...\n5| public class Deco {\n8|     public String toString() {\n9|         \
                    return \"\";\n...";
    assert_eq!(sliced(Language::Java, java, 9), expected);
    let python = "class Point:\n    @property\n    def norm(self):\n        return 0\n";
    let expected = "...\n1| class Point:\n3|     def norm(self):\n4|         return 0";
    assert_eq!(sliced(Language::Python, python, 4), expected);
}
