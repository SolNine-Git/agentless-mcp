use syntax::{DecisionRules, FieldCursor, KindId, NodeId, Parsed, Spec, UnitRules};

use crate::kinds::Kinds;
use crate::{Unit, line, lines_within, stream};

struct Found {
    node: NodeId,
    qualname: String,
    name: NodeId,
}

pub(crate) fn units(parsed: &Parsed, spec: &Spec, kinds: &Kinds, sloc_lines: &[u32]) -> Vec<Unit> {
    // The walk and then each unit's decision count read fields in about document order, the order
    // in which one cursor moves cheaply.
    let mut fields = FieldCursor::new(parsed);
    walk(&mut fields, &spec.units, kinds)
        .into_iter()
        .map(|found| {
            let start_line = line(parsed.start_point(found.node));
            let end_line = line(parsed.end_point(found.node));
            Unit {
                start_byte: parsed.start_byte(found.node),
                end_byte: parsed.end_byte(found.node),
                start_line,
                body_key: stream::body_key(parsed, found.node, found.name),
                cc: 1 + decisions(&mut fields, &spec.decisions, found.node),
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
fn walk(fields: &mut FieldCursor<'_>, rules: &UnitRules, kinds: &Kinds) -> Vec<Found> {
    let parsed = fields.parsed();
    let mut found = Vec::new();
    let mut stack = vec![(parsed.root(), String::new())];
    while let Some((node, prefix)) = stack.pop() {
        let mut entered = Vec::new();
        for child in parsed.named_children(node) {
            let Some((inner, name)) = step(fields, rules, kinds, child, &prefix) else {
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
    fields: &mut FieldCursor<'_>,
    rules: &UnitRules,
    kinds: &Kinds,
    child: NodeId,
    prefix: &str,
) -> Option<(String, Option<NodeId>)> {
    let parsed = fields.parsed();
    if let Some((name, text)) = unit_name(fields, rules, kinds, child) {
        let owner = rules
            .owner_of(fields, child)
            .map_or_else(String::new, |owner| name_text(parsed, kinds, owner));
        return Some((join(&[prefix, &owner, &text]), Some(name)));
    }
    let kind = parsed.kind(child);
    if rules.scopes.contains(kind) {
        let scoped = match rules.name_of(fields, child) {
            Some(scope) => join(&[prefix, &name_text(parsed, kinds, scope)]),
            None => prefix.to_owned(),
        };
        return Some((scoped, None));
    }
    rules.descends_into(kind).then(|| (prefix.to_owned(), None))
}

fn unit_name(
    fields: &mut FieldCursor<'_>,
    rules: &UnitRules,
    kinds: &Kinds,
    node: NodeId,
) -> Option<(NodeId, String)> {
    let parsed = fields.parsed();
    let kind = parsed.kind(node);
    let name = if rules.units.contains(kind) {
        rules.name_of(fields, node)
    } else if rules.bound.contains(kind) {
        rules
            .name_of(fields, node)
            .or_else(|| rules.binding_of(fields, node))
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
fn decisions(fields: &mut FieldCursor<'_>, rules: &DecisionRules, unit: NodeId) -> u32 {
    let parsed = fields.parsed();
    parsed
        .descendants(unit)
        .filter(|&node| parsed.is_named(node))
        .map(|node| weight(fields, rules, node))
        .sum()
}

fn weight(fields: &mut FieldCursor<'_>, rules: &DecisionRules, node: NodeId) -> u32 {
    let parsed = fields.parsed();
    let kind = parsed.kind(node);
    u32::from(rules.points.contains(kind))
        + u32::from(rules.condition(fields, node) == Some(true))
        + operator_weight(fields, rules, node, kind)
        + token_weight(parsed, rules, node, kind)
}

fn operator_weight(
    fields: &mut FieldCursor<'_>,
    rules: &DecisionRules,
    node: NodeId,
    kind: KindId,
) -> u32 {
    let Some(spellings) = rules.operator_spellings(kind) else {
        return 0;
    };
    let operator = rules
        .operator_field()
        .and_then(|field| fields.child_by_field(node, field));
    u32::from(operator.is_some_and(|operator| spellings.contains(&fields.parsed().text(operator))))
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
