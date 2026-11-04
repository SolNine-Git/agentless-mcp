use app::{PathHop, PathPolicy, PathTrace};

use super::path_text;

fn hop(arrow: &'static str, verb: &'static str, node: &str, label: &str, line: u32) -> PathHop {
    PathHop {
        verb,
        tier: "imported",
        tier_label: "resolved-via-import",
        arrow,
        node: node.to_owned(),
        label: label.to_owned(),
        path: node
            .rsplit_once("::")
            .map_or(node, |(path, _)| path)
            .to_owned(),
        line,
    }
}

fn trace(source: (&str, &str), target: &str, hops: Vec<PathHop>) -> PathTrace {
    PathTrace {
        source: source.0.to_owned(),
        target: target.to_owned(),
        source_label: source.1.to_owned(),
        target_label: target.to_owned(),
        hops,
        found: true,
        message: String::new(),
        visited: 2,
        exhausted: false,
        policy: PathPolicy::default(),
        endpoints_resolved: true,
    }
}

#[test]
fn a_file_row_names_its_path_once_and_a_row_with_no_line_prints_no_line() {
    let hops = vec![
        hop(
            "<-",
            "imported by",
            "src/LegacyCallHandler.tsx",
            "src/LegacyCallHandler.tsx",
            55,
        ),
        hop("->", "imports", "src/SdkConfig.ts", "src/SdkConfig.ts", 0),
    ];
    let source = ("src/createRoom.ts", "src/createRoom.ts");
    let text = path_text(&trace(source, "src/SdkConfig.ts", hops));
    assert_eq!(
        text,
        "2 hops from src/createRoom.ts to src/SdkConfig.ts\n  start  src/createRoom.ts\n    1. <- \
         imported by (resolved-via-import)    [src/LegacyCallHandler.tsx] @55\n    2. -> imports \
         (resolved-via-import)    [src/SdkConfig.ts]\n"
    );
}

#[test]
fn a_symbol_row_keeps_its_label_its_id_and_its_line() {
    let hops = vec![hop("->", "references", "py:b.py::g", "g", 3)];
    let text = path_text(&trace(("py:a.py::f", "f"), "py:b.py::g", hops));
    assert_eq!(
        text,
        "1 hop from py:a.py::f to py:b.py::g\n  start  f    py:a.py::f\n    1. -> references \
         (resolved-via-import)    g    [py:b.py::g] @3\n"
    );
}
