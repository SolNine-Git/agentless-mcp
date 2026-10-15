use syntax::{DecisionRules, KindId, NodeId, Parsed, Spec, UnitRules};

use crate::kinds::Kinds;
use crate::{Unit, line, lines_within, stream};

struct Found {
    node: NodeId,
    qualname: String,
    name: NodeId,
}

pub(crate) fn units(parsed: &Parsed, spec: &Spec, kinds: &Kinds, sloc_lines: &[u32]) -> Vec<Unit> {
    walk(parsed, &spec.units, kinds)
        .into_iter()
        .map(|found| {
            let start_line = line(parsed.start_point(found.node));
            let end_line = line(parsed.end_point(found.node));
            Unit {
                start_byte: parsed.start_byte(found.node),
                end_byte: parsed.end_byte(found.node),
                start_line,
                body_key: stream::body_key(parsed, found.node, found.name),
                cc: 1 + decisions(parsed, &spec.decisions, found.node),
                sloc: count(lines_within(sloc_lines, start_line, end_line)),
                qualname: found.qualname,
            }
        })
        .collect()
}

// The lines are distinct and at least 1, so a count of them inside one span fits u32.
fn count(lines: usize) -> u32 {
    u32::try_from(lines).unwrap_or(u32::MAX)
}

// The units in start-byte order. A stable sort keeps the walk order of units that start together.
fn walk(parsed: &Parsed, rules: &UnitRules, kinds: &Kinds) -> Vec<Found> {
    let mut found = Vec::new();
    let mut stack = vec![(parsed.root(), String::new())];
    while let Some((node, prefix)) = stack.pop() {
        let mut entered = Vec::new();
        for child in parsed.named_children(node) {
            let Some((inner, name)) = step(parsed, rules, kinds, child, &prefix) else {
                continue;
            };
            if let Some(name) = name {
                let qualname = inner.clone();
                found.push(Found {
                    node: child,
                    qualname,
                    name,
                });
            }
            entered.push((child, inner));
        }
        stack.extend(entered.into_iter().rev());
    }
    found.sort_by_key(|found| parsed.start_byte(found.node));
    found
}

// The prefix to enter `child` with, and its name node when `child` is a unit.
fn step(
    parsed: &Parsed,
    rules: &UnitRules,
    kinds: &Kinds,
    child: NodeId,
    prefix: &str,
) -> Option<(String, Option<NodeId>)> {
    if let Some((name, text)) = unit_name(parsed, rules, kinds, child) {
        let owner = rules
            .owner_of(parsed, child)
            .map_or_else(String::new, |owner| name_text(parsed, kinds, owner));
        return Some((join(&[prefix, &owner, &text]), Some(name)));
    }
    let kind = parsed.kind(child);
    if rules.scopes.contains(kind) {
        let scoped = match rules.name_of(parsed, child) {
            Some(scope) => join(&[prefix, &name_text(parsed, kinds, scope)]),
            None => prefix.to_owned(),
        };
        return Some((scoped, None));
    }
    rules.descends_into(kind).then(|| (prefix.to_owned(), None))
}

fn unit_name(
    parsed: &Parsed,
    rules: &UnitRules,
    kinds: &Kinds,
    node: NodeId,
) -> Option<(NodeId, String)> {
    let kind = parsed.kind(node);
    let name = if rules.units.contains(kind) {
        rules.name_of(parsed, node)
    } else if rules.bound.contains(kind) {
        rules
            .name_of(parsed, node)
            .or_else(|| rules.binding_of(parsed, node))
    } else {
        None
    }?;
    let text = name_text(parsed, kinds, name);
    (!text.is_empty()).then_some((name, text))
}

// A string name is the text of its fragments, and `::` reads as a dot.
fn name_text(parsed: &Parsed, kinds: &Kinds, node: NodeId) -> String {
    let text = if kinds.string.contains(parsed.kind(node)) {
        parsed
            .named_children(node)
            .filter(|&part| kinds.string_fragment.contains(parsed.kind(part)))
            .map(|part| parsed.text(part))
            .collect()
    } else {
        parsed.text(node).to_owned()
    };
    text.replace("::", ".")
}

fn join(parts: &[&str]) -> String {
    let kept: Vec<&str> = parts
        .iter()
        .copied()
        .filter(|part| !part.is_empty())
        .collect();
    kept.join(".")
}

// Ruby spells keywords such as `if` and `when` like the named nodes they open, so only named
// nodes are weighed.
fn decisions(parsed: &Parsed, rules: &DecisionRules, unit: NodeId) -> u32 {
    parsed
        .descendants(unit)
        .filter(|&node| parsed.is_named(node))
        .map(|node| weight(parsed, rules, node))
        .sum()
}

fn weight(parsed: &Parsed, rules: &DecisionRules, node: NodeId) -> u32 {
    let kind = parsed.kind(node);
    u32::from(rules.points.contains(kind))
        + u32::from(rules.condition(parsed, node) == Some(true))
        + operator_weight(parsed, rules, node, kind)
        + token_weight(parsed, rules, node, kind)
}

fn operator_weight(parsed: &Parsed, rules: &DecisionRules, node: NodeId, kind: KindId) -> u32 {
    let Some(spellings) = rules.operator_spellings(kind) else {
        return 0;
    };
    let operator = rules
        .operator_field()
        .and_then(|field| parsed.child_by_field(node, field));
    u32::from(operator.is_some_and(|operator| spellings.contains(&parsed.text(operator))))
}

fn token_weight(parsed: &Parsed, rules: &DecisionRules, node: NodeId, kind: KindId) -> u32 {
    let Some(tokens) = rules.token_operators(kind) else {
        return 0;
    };
    parsed
        .children(node)
        .filter(|&child| !parsed.is_named(child) && tokens.contains(parsed.kind(child)))
        .map(|_| 1)
        .sum()
}
