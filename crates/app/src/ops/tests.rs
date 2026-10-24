use std::collections::BTreeSet;

use super::limits::{DEFAULT_MAX_FILES, DEFAULT_RESOLUTION, MAX_HTML_EDGES, MAX_HTML_NODES};
use super::table::KIND_NAMES;
use super::{Door, OPS, Param, Value, op};

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

fn tools() -> Vec<(String, Vec<String>)> {
    io::testing::cases(MANIFEST_DIR, "read/doors", "tools")
}

// The name the MCP tool gives a parameter, or None when only the command line has it.
fn mcp_name(param: &Param) -> Option<&'static str> {
    match param.door {
        Door::Same => Some(param.dest),
        Door::Named(name) => Some(name),
        Door::CliOnly => None,
    }
}

#[test]
fn each_mcp_name_is_a_parameter_of_the_operation_tool() {
    let tools = tools();
    for op in OPS.iter().filter(|op| op.tool.is_some()) {
        let tool = op.tool.unwrap_or_default();
        let (_, names) = tools
            .iter()
            .find(|(name, _)| name == tool)
            .unwrap_or_else(|| panic!("{}: no tool {tool}", op.name));
        for name in op.params.iter().filter_map(mcp_name) {
            assert!(
                names.iter().any(|n| n == name),
                "{}: {tool} has no {name}",
                op.name
            );
        }
    }
}

// The tool's operation selector is the one name no command-line parameter stands for.
#[test]
fn each_parameter_of_a_used_tool_is_declared_by_an_operation() {
    for (tool, names) in tools() {
        let ops: Vec<_> = OPS
            .iter()
            .filter(|op| op.tool == Some(tool.as_str()))
            .collect();
        if ops.is_empty() {
            continue;
        }
        let declared: BTreeSet<&str> = ops
            .iter()
            .flat_map(|op| op.params.iter().filter_map(mcp_name))
            .collect();
        for name in names.iter().filter(|name| *name != "operation") {
            assert!(
                declared.contains(name.as_str()),
                "{tool}: {name} is not declared"
            );
        }
    }
}

#[test]
fn the_kind_choices_are_the_symbol_kinds_in_order() {
    let kinds = facts::SymbolKind::ALL.map(facts::SymbolKind::as_str);
    assert_eq!(KIND_NAMES, kinds);
}

// The number a help text spells after `marker`, as written.
fn spelled_after<'h>(help: &'h str, marker: &str) -> Option<&'h str> {
    let (_, rest) = help.split_once(marker)?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

// The default that the operation applies, for each parameter whose help names one.
fn declared_default(op: &str, param: &Param) -> Option<String> {
    match (op, param.dest, param.value) {
        (_, _, Value::Int(default)) => Some(default.to_string()),
        ("map", "max_files", Value::IntOrUnset) => Some(DEFAULT_MAX_FILES.to_string()),
        (_, "resolution", Value::Float) => Some(DEFAULT_RESOLUTION.to_string()),
        _ => None,
    }
}

// The bound that the operation checks, for each parameter whose help names one.
fn declared_maximum(op: &str, dest: &str) -> Option<String> {
    match (op, dest) {
        ("html", "max_nodes") => Some(MAX_HTML_NODES.to_string()),
        ("html", "max_edges") => Some(MAX_HTML_EDGES.to_string()),
        _ => None,
    }
}

#[test]
fn each_help_default_and_maximum_is_the_declared_value() {
    let mut checked = 0;
    for op in &OPS {
        for param in op.params {
            let help = param.help.unwrap_or_default();
            let at = format!("{} {}", op.name, param.dest);
            if let Some(shown) = spelled_after(help, "(default: ") {
                assert_eq!(
                    Some(shown.to_owned()),
                    declared_default(op.name, param),
                    "{at}"
                );
                checked += 1;
            }
            if let Some(shown) = spelled_after(help, "maximum: ") {
                assert_eq!(
                    Some(shown.to_owned()),
                    declared_maximum(op.name, param.dest),
                    "{at}"
                );
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 16);
}

#[test]
fn each_operation_is_found_by_its_unique_name() {
    let names: BTreeSet<&str> = OPS.iter().map(|op| op.name).collect();
    assert_eq!(names.len(), OPS.len());
    for declared in OPS {
        assert!(op(declared.name).is_some_and(|found| std::ptr::eq(found, declared)));
    }
    assert!(op("lint").is_none());
}
