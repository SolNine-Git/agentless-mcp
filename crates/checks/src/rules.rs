//! The contract rules of one language, compiled once per run, and the hits they find in a tree.

use std::collections::HashMap;

use metrics::Unit;
use prompts::ContractRule;
use syntax::{KindSet, Language, NodeId, Parsed, Query};

use crate::error::Error;

const HIT_CAPTURE: &str = "hit";

// The rules of one language as one query, and the rule of each of its patterns.
pub(crate) struct CompiledRules {
    rules: Vec<&'static ContractRule>,
    query: Query,
    pattern_rules: Vec<usize>,
    hit: Option<u32>,
}

pub(crate) struct Hit<'r> {
    pub(crate) rule: &'r ContractRule,
    pub(crate) unit: Option<usize>,
    pub(crate) line: u32,
    pub(crate) capture: String,
}

pub(crate) fn rules_for(language: Language) -> impl Iterator<Item = &'static ContractRule> {
    prompts::contract_rules()
        .iter()
        .filter(move |rule| rule.language == language.name())
}

// The rule table is static, and the tests check each rule's query, captures and predicates.
pub(crate) fn compile(language: Language) -> CompiledRules {
    let rules: Vec<&'static ContractRule> = rules_for(language).collect();
    let mut joined = String::new();
    let mut starts = Vec::with_capacity(rules.len());
    for rule in &rules {
        starts.push(joined.len());
        joined.push_str(rule.query);
        joined.push('\n');
    }
    let query = Query::new(language, &joined).expect("the static contract rules compile");
    let pattern_rules = (0..query.pattern_count())
        .map(|pattern| rule_at(&starts, query.start_byte_for_pattern(pattern)))
        .collect();
    let hit = query
        .capture_names()
        .iter()
        .position(|name| *name == HIT_CAPTURE)
        .and_then(|index| u32::try_from(index).ok());
    CompiledRules {
        rules,
        query,
        pattern_rules,
        hit,
    }
}

// The rule whose text holds `offset`: the last one that starts at or before it.
fn rule_at(starts: &[usize], offset: usize) -> usize {
    starts.partition_point(|&start| start <= offset) - 1
}

pub(crate) fn hits<'r>(
    parsed: &Parsed,
    path: &str,
    units: &[Unit],
    compiled: &'r CompiledRules,
) -> Result<Vec<Hit<'r>>, Error> {
    let comments = parsed.language().spec().map(|spec| &spec.comments);
    let mut found = Vec::new();
    for (order, nodes) in hit_nodes(parsed, path, compiled)?.into_iter().enumerate() {
        for node in nodes {
            let start = parsed.start_byte(node);
            let hit = Hit {
                rule: compiled.rules[order],
                unit: enclosing(units, start),
                line: parsed.start_point(node).row + 1,
                capture: capture(parsed, node, comments),
            };
            found.push((start, order, hit));
        }
    }
    found.sort_by_key(|(start, order, _)| (*start, *order));
    Ok(found.into_iter().map(|(_, _, hit)| hit).collect())
}

// The hit nodes of each rule in rule order: one node per span, by span.
fn hit_nodes(
    parsed: &Parsed,
    path: &str,
    compiled: &CompiledRules,
) -> Result<Vec<Vec<NodeId>>, Error> {
    let Some(first) = compiled.rules.first() else {
        return Ok(Vec::new());
    };
    // On a tree of another language, the first rule's own query was the first to fail.
    let matches = parsed
        .matches(&compiled.query)
        .map_err(|source| Error::Query {
            id: first.id,
            language: first.language,
            path: path.to_owned(),
            source,
        })?;
    let mut spans: Vec<HashMap<(u32, u32), NodeId>> = vec![HashMap::new(); compiled.rules.len()];
    for found in &matches {
        let rule = &mut spans[compiled.pattern_rules[found.pattern as usize]];
        let hits = found
            .captures
            .iter()
            .filter(|capture| Some(capture.index) == compiled.hit);
        for capture in hits {
            let node = capture.node;
            let span = (parsed.start_byte(node), parsed.end_byte(node));
            rule.entry(span).or_insert(node);
        }
    }
    Ok(spans.into_iter().map(by_span).collect())
}

fn by_span(spans: HashMap<(u32, u32), NodeId>) -> Vec<NodeId> {
    let mut ordered: Vec<((u32, u32), NodeId)> = spans.into_iter().collect();
    ordered.sort_by_key(|(span, _)| *span);
    ordered.into_iter().map(|(_, node)| node).collect()
}

// The innermost unit that holds the offset: the greatest start, the first one on a tie.
fn enclosing(units: &[Unit], offset: u32) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (index, unit) in units.iter().enumerate() {
        let holds = unit.start_byte <= offset && offset < unit.end_byte;
        if holds && best.is_none_or(|kept| unit.start_byte > units[kept].start_byte) {
            best = Some(index);
        }
    }
    best
}

// Comments are cut out so that editing one does not turn an old hit into a new one.
fn capture(parsed: &Parsed, node: NodeId, comments: Option<&KindSet>) -> String {
    let source = parsed.source();
    let mut kept = String::new();
    let mut cursor = parsed.start_byte(node) as usize;
    for comment in comment_nodes(parsed, node, comments) {
        let range = parsed.byte_range(comment);
        kept.push_str(&source[cursor..range.start]);
        cursor = range.end;
    }
    kept.push_str(&source[cursor..parsed.end_byte(node) as usize]);
    text::split_whitespace(&kept).collect::<Vec<_>>().join(" ")
}

fn comment_nodes(parsed: &Parsed, node: NodeId, comments: Option<&KindSet>) -> Vec<NodeId> {
    let Some(comments) = comments else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if comments.contains(parsed.kind(current)) {
            found.push(current);
            continue;
        }
        let children: Vec<NodeId> = parsed.children(current).collect();
        stack.extend(children.into_iter().rev());
    }
    found
}

#[cfg(test)]
mod tests;
