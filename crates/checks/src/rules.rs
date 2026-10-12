//! The contract rules of one language, compiled once per run, and the hits they find in a tree.

use std::collections::HashMap;

use metrics::Unit;
use prompts::ContractRule;
use syntax::{KindSet, Language, NodeId, Parsed, Query};

use crate::error::{Error, RuleProblem};

const HIT_CAPTURE: &str = "hit";
// An unknown predicate compiles and is then ignored, so a typo would widen a rule.
const PREDICATES: [&str; 6] = [
    "eq?",
    "not-eq?",
    "any-of?",
    "not-any-of?",
    "match?",
    "not-match?",
];

pub(crate) struct CompiledRule {
    pub(crate) rule: &'static ContractRule,
    query: Query,
    hit: u32,
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

pub(crate) fn compile(language: Language) -> Result<Vec<CompiledRule>, Error> {
    rules_for(language)
        .map(|rule| compile_rule(rule, language))
        .collect()
}

fn compile_rule(rule: &'static ContractRule, language: Language) -> Result<CompiledRule, Error> {
    let refused = |problem| Error::Rule {
        id: rule.id,
        language: rule.language,
        problem,
    };
    let query =
        Query::new(language, rule.query).map_err(|error| refused(RuleProblem::Compile(error)))?;
    let names = query.capture_names();
    let hit = names.iter().position(|name| *name == HIT_CAPTURE);
    let stray = names
        .iter()
        .any(|name| *name != HIT_CAPTURE && !name.starts_with('_'));
    let (Some(hit), false) = (hit, stray) else {
        return Err(refused(RuleProblem::Captures));
    };
    let unknown = unknown_predicates(rule.query);
    if !unknown.is_empty() {
        return Err(refused(RuleProblem::Predicates(unknown)));
    }
    let hit = u32::try_from(hit).map_err(|_| refused(RuleProblem::Captures))?;
    Ok(CompiledRule { rule, query, hit })
}

// The names that `\(#([A-Za-z-]+[?!])` finds, sorted, without repeats, and outside PREDICATES.
fn unknown_predicates(query: &str) -> Vec<String> {
    let mut unknown: Vec<String> = predicate_names(query)
        .into_iter()
        .filter(|name| !PREDICATES.contains(name))
        .map(str::to_owned)
        .collect();
    unknown.sort();
    unknown.dedup();
    unknown
}

fn predicate_names(query: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut rest = query;
    while let Some(at) = rest.find("(#") {
        let after = &rest[at + 2..];
        let run = after.len()
            - after
                .trim_start_matches(|c: char| c.is_ascii_alphabetic() || c == '-')
                .len();
        if run > 0 && matches!(after.as_bytes().get(run), Some(b'?' | b'!')) {
            names.push(&after[..=run]);
            rest = &after[run + 1..];
        } else {
            rest = &rest[at + 1..];
        }
    }
    names
}

pub(crate) fn hits<'r>(
    parsed: &Parsed,
    path: &str,
    units: &[Unit],
    rules: &'r [CompiledRule],
) -> Result<Vec<Hit<'r>>, Error> {
    let comments = parsed.language().spec().map(|spec| &spec.comments);
    let mut found = Vec::new();
    for (order, entry) in rules.iter().enumerate() {
        for node in hit_nodes(parsed, path, entry)? {
            let start = parsed.start_byte(node);
            let hit = Hit {
                rule: entry.rule,
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

fn hit_nodes(parsed: &Parsed, path: &str, entry: &CompiledRule) -> Result<Vec<NodeId>, Error> {
    let matches = parsed
        .matches(&entry.query)
        .map_err(|source| Error::Query {
            id: entry.rule.id,
            language: entry.rule.language,
            path: path.to_owned(),
            source,
        })?;
    let mut spans: HashMap<(u32, u32), NodeId> = HashMap::new();
    for capture in matches.iter().flat_map(|found| &found.captures) {
        if capture.index == entry.hit {
            let node = capture.node;
            let span = (parsed.start_byte(node), parsed.end_byte(node));
            spans.entry(span).or_insert(node);
        }
    }
    let mut ordered: Vec<((u32, u32), NodeId)> = spans.into_iter().collect();
    ordered.sort_by_key(|(span, _)| *span);
    Ok(ordered.into_iter().map(|(_, node)| node).collect())
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
    pycompat::split_whitespace(&kept)
        .collect::<Vec<_>>()
        .join(" ")
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
