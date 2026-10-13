use std::collections::BTreeSet;

use super::table::KIND_NAMES;
use super::{Door, OPS, Param, op};

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

#[test]
fn each_operation_is_found_by_its_unique_name() {
    let names: BTreeSet<&str> = OPS.iter().map(|op| op.name).collect();
    assert_eq!(names.len(), OPS.len());
    for declared in &OPS {
        assert!(op(declared.name).is_some_and(|found| std::ptr::eq(found, declared)));
    }
    assert!(op("lint").is_none());
}
