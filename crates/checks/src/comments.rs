//! `comment_delta`: each comment finding that a change adds: a lint suppression, a banner, a
//! comment over 2 lines, or a doc that the doc rules forbid.

use std::collections::{BTreeSet, HashMap};

use prompts::{
    CommentGroup, CommentRules, Declared, DocRules, Head, RunRules, Suppressed, SuppressionRules,
    messages,
};
use regex::Regex;
use syntax::{FieldId, KindSet, Language, NodeId, Parsed};

use crate::error::Error;
use crate::finding::{CheckGap, CommentKey, Detail, Finding, PairFinding, Severity};
use crate::finding::{not_checked, sorted};
use crate::observed::{CommentState, Observed, Outcome};
use crate::pair::{Pair, Read, Side, each_pair};

mod rust;

const MAX_COMMENT_LINES: usize = 2;
const SUPPRESS: &str = "suppress";
const BANNER: &str = "banner";
const RUN: &str = "run";
const PRIVATE_DOC: &str = "private-doc";
const SECTION: &str = "section";

/// Report each comment finding whose key a change adds to a file, so a moved comment is no finding.
///
/// # Errors
///
/// Returns an error when two pairs name one path, or when a side gives no tree.
pub fn comment_delta(pairs: &[&Pair]) -> Result<Outcome, Error> {
    let mut compiled: HashMap<Language, Option<Compiled>> = HashMap::new();
    let (read, mut findings) = each_pair(pairs, |pair, language| {
        let rules = rules_for(language);
        let gaps = gap_rows(pair, language, rules);
        let compiled = compiled
            .entry(language)
            .or_insert_with(|| compile(language, rules));
        let compared = compiled
            .as_ref()
            .map(|compiled| compare(pair, language, compiled))
            .transpose()?;
        Ok(Read::Value((gaps, compared)))
    })?;
    let mut states: Option<Vec<CommentState>> = None;
    for (gaps, compared) in read {
        findings.extend(gaps);
        if let Some(compared) = compared {
            findings.extend(compared.fresh);
            states.get_or_insert_with(Vec::new).extend(compared.states);
        }
    }
    if let Some(states) = &mut states {
        states.sort_by(|a, b| a.path.cmp(&b.path));
    }
    Ok(Outcome {
        findings: sorted(findings),
        observed: Observed {
            comment_keys: states,
            ..Observed::default()
        },
    })
}

// The coverage test requires a row for every language file, so every language has one.
fn rules_for(language: Language) -> &'static CommentRules {
    prompts::comment_rules()
        .iter()
        .find(|rules| rules.language == language.name())
        .expect("every language has a comment rule row")
}

fn gap_rows(pair: &Pair, language: Language, rules: &CommentRules) -> Vec<PairFinding> {
    if !pair.has_new() {
        return Vec::new();
    }
    let row = |group: CommentGroup| {
        let gap = rules.gap(group)?;
        let values = (pair.path(), group.name(), language.name(), gap.name());
        let message = messages::COMMENT_NO_RULES.render(values);
        let detail = Some(CheckGap::NoCommentRules(language, group));
        let check = crate::COMMENT_DELTA;
        Some(not_checked(check, pair.path(), message, gap.name(), detail))
    };
    CommentGroup::ALL.into_iter().filter_map(row).collect()
}

// One language's rules with each pattern compiled.
struct Compiled {
    line: Regex,
    block: Option<Regex>,
    doc: Option<Regex>,
    head: Head,
    directive: Option<Regex>,
    banner: Regex,
    markers: Markers,
    docs: Option<Docs>,
    suppression: Option<Suppression>,
}

enum Docs {
    Docstring {
        private: Regex,
        sections: Regex,
        kinds: Box<DocstringKinds>,
    },
    Jsdoc {
        tags: Regex,
        aliases: &'static [(&'static str, &'static str)],
    },
    RustItems(Box<rust::RustItems>),
}

enum Suppression {
    Comment(Regex),
    Attribute(Regex),
}

// The kinds of the child that makes a comment node a doc comment, in a grammar that has them.
struct Markers {
    outer: KindSet,
    inner: KindSet,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Marker {
    Outer,
    Inner,
}

// The Python kinds and fields that hold a function's docstring.
struct DocstringKinds {
    functions: KindSet,
    strings: KindSet,
    joined: KindSet,
    comments: KindSet,
    name: Option<FieldId>,
    body: Option<FieldId>,
}

// The rule table is static, and a test compiles every pattern of it.
fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("the comment rule patterns compile")
}

fn compile(language: Language, rules: &CommentRules) -> Option<Compiled> {
    let Declared::Rules(runs) = rules.runs else {
        return None;
    };
    let RunRules {
        line,
        block,
        doc,
        head,
        directive,
        banner,
    } = runs;
    Some(Compiled {
        line: regex(line),
        block: block.map(regex),
        doc: doc.map(regex),
        head,
        directive: directive.map(regex),
        banner: regex(banner),
        markers: Markers {
            outer: language.kinds(&["outer_doc_comment_marker"]),
            inner: language.kinds(&["inner_doc_comment_marker"]),
        },
        docs: compiled_docs(language, rules.docs),
        suppression: compiled_suppression(rules.suppressions),
    })
}

fn compiled_docs(language: Language, docs: Declared<DocRules>) -> Option<Docs> {
    match docs {
        Declared::Gap(_) => None,
        Declared::Rules(DocRules::Docstring { private, sections }) => Some(Docs::Docstring {
            private: regex(private),
            sections: regex(sections),
            kinds: Box::new(docstring_kinds(language)),
        }),
        Declared::Rules(DocRules::Jsdoc { tags, aliases }) => Some(Docs::Jsdoc {
            tags: regex(tags),
            aliases,
        }),
        Declared::Rules(DocRules::RustItems {
            item,
            public,
            export,
            facing,
            sections,
        }) => {
            let rules = rust::RustItems::new(item, public, export, facing, sections);
            Some(Docs::RustItems(Box::new(rules)))
        }
    }
}

fn compiled_suppression(suppressions: Declared<SuppressionRules>) -> Option<Suppression> {
    let Declared::Rules(SuppressionRules { on, pattern }) = suppressions else {
        return None;
    };
    Some(match on {
        Suppressed::Comment => Suppression::Comment(regex(pattern)),
        Suppressed::Attribute => Suppression::Attribute(regex(pattern)),
    })
}

fn docstring_kinds(language: Language) -> DocstringKinds {
    let comments = language
        .spec()
        .map_or_else(|| language.kinds(&[]), |spec| spec.comments.clone());
    DocstringKinds {
        functions: language.kinds(&["function_definition"]),
        strings: language.kinds(&["string"]),
        joined: language.kinds(&["concatenated_string"]),
        comments,
        name: language.field("name"),
        body: language.field("body"),
    }
}

struct Compared {
    fresh: Vec<PairFinding>,
    states: Vec<CommentState>,
}

fn compare(pair: &Pair, language: Language, compiled: &Compiled) -> Result<Compared, Error> {
    let found = |side: Option<&Side>| -> Result<Vec<Found>, Error> {
        let Some(side) = side else {
            return Ok(Vec::new());
        };
        let tree = side.tree(language).map_err(|source| Error::Side {
            path: pair.path().to_owned(),
            source,
        })?;
        Ok(side_findings(tree.parsed(), compiled))
    };
    let old = found(pair.old_side())?;
    let new = found(pair.new_side())?;
    let path = pair.path();
    let states = new
        .iter()
        .map(|found| CommentState {
            path: path.to_owned(),
            rule: found.rule.to_owned(),
            key: found.key.clone(),
        })
        .collect();
    let fresh = fresh(&old, new)
        .into_iter()
        .map(|found| finding(path, found))
        .collect();
    Ok(Compared { fresh, states })
}

// The new findings whose key the old text does not hold as many times, in the new text's order.
fn fresh(old: &[Found], new: Vec<Found>) -> Vec<Found> {
    let mut carried: HashMap<&str, HashMap<&str, usize>> = HashMap::new();
    for found in old {
        let keys = carried.entry(found.rule).or_default();
        *keys.entry(&found.key).or_default() += 1;
    }
    let mut kept = Vec::new();
    for found in new {
        let keys = carried.get_mut(found.rule);
        let count = keys.and_then(|keys| keys.get_mut(found.key.as_str()));
        if let Some(count) = count.filter(|count| **count > 0) {
            *count -= 1;
        } else {
            kept.push(found);
        }
    }
    kept
}

fn finding(path: &str, found: Found) -> PairFinding {
    let finding = Finding {
        check: crate::COMMENT_DELTA,
        severity: Severity::Warning,
        message: found.message,
        path: path.to_owned(),
        line: found.line,
        evidence: messages::COMMENT_EVIDENCE.render(found.rule),
    };
    let key = CommentKey {
        rule: found.rule.to_owned(),
        key: found.key,
    };
    PairFinding {
        finding,
        detail: Some(Detail::Comment(key)),
    }
}

// One finding of one text: its rule and key, which compare across texts, and its line.
struct Found {
    line: u32,
    rule: &'static str,
    key: String,
    message: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    Line,
    Block,
    Doc,
    Other,
}

// A comment with no trailing newline: its first and last line, and its byte range.
struct Comment<'t> {
    line: u32,
    end: u32,
    start: usize,
    stop: usize,
    text: &'t str,
    head: &'t str,
    own_line: bool,
    marker: Option<Marker>,
    form: Form,
}

// The comment findings in comment order, then the runs, then the doc findings: the reference's
// order within each rule, which decides the line of a repeated key.
fn side_findings(parsed: &Parsed, compiled: &Compiled) -> Vec<Found> {
    let comments = comments(parsed, compiled);
    let mut found: Vec<Found> = comments
        .iter()
        .filter_map(|comment| comment_finding(comment, compiled))
        .collect();
    found.extend(line_runs(&comments, compiled));
    if let Some(Docs::Docstring {
        private,
        sections,
        kinds,
    }) = &compiled.docs
    {
        found.extend(docstring_findings(parsed, kinds, private, sections));
    }
    found.extend(code_findings(parsed, &comments, compiled));
    found
}

// The rules that read the source with its comments and strings blanked, built once for all of them.
fn code_findings(parsed: &Parsed, comments: &[Comment<'_>], compiled: &Compiled) -> Vec<Found> {
    let items = match &compiled.docs {
        Some(Docs::RustItems(rules)) => Some(rules),
        _ => None,
    };
    let attributes = match &compiled.suppression {
        Some(Suppression::Attribute(pattern)) => Some(pattern),
        _ => None,
    };
    if items.is_none() && attributes.is_none() {
        return Vec::new();
    }
    let code = rust::code(parsed);
    let mut found = items.map_or_else(Vec::new, |rules| rules.findings(comments, &code));
    if let Some(pattern) = attributes {
        found.extend(rust::attribute_findings(parsed.source(), &code, pattern));
    }
    found
}

fn comments<'t>(parsed: &'t Parsed, compiled: &Compiled) -> Vec<Comment<'t>> {
    let Some(spec) = parsed.language().spec() else {
        return Vec::new();
    };
    let source = parsed.source();
    let nodes = parsed.descendants(parsed.root());
    let comments = nodes.filter(|node| spec.comments.contains(parsed.kind(*node)));
    comments
        .map(|node| {
            let text = parsed.text(node);
            let text = text.strip_suffix('\n').unwrap_or(text);
            let start = parsed.start_byte(node) as usize;
            let line_start = source[..start].rfind('\n').map_or(0, |at| at + 1);
            let line = parsed.start_point(node).row + 1;
            let lines = u32::try_from(text.matches('\n').count()).unwrap_or(u32::MAX);
            let marker = marker(parsed, node, &compiled.markers);
            Comment {
                line,
                end: line.saturating_add(lines),
                start,
                stop: start + text.len(),
                text,
                head: head(text, compiled.head),
                own_line: text::strip(&source[line_start..start]).is_empty(),
                marker,
                form: form(text, marker, compiled),
            }
        })
        .collect()
}

fn marker(parsed: &Parsed, node: NodeId, markers: &Markers) -> Option<Marker> {
    parsed.children(node).find_map(|child| {
        let kind = parsed.kind(child);
        if markers.outer.contains(kind) {
            Some(Marker::Outer)
        } else if markers.inner.contains(kind) {
            Some(Marker::Inner)
        } else {
            None
        }
    })
}

fn head(text: &str, head: Head) -> &str {
    match head {
        Head::Comment => text,
        Head::FirstLine => text::splitlines(text, false)
            .first()
            .map_or("", |line| text::strip(line)),
    }
}

fn form(text: &str, marker: Option<Marker>, compiled: &Compiled) -> Form {
    let matches = |pattern: &Option<Regex>| pattern.as_ref().is_some_and(|p| p.is_match(text));
    if marker.is_some() || matches(&compiled.doc) {
        Form::Doc
    } else if matches(&compiled.block) {
        Form::Block
    } else if compiled.line.is_match(text) {
        Form::Line
    } else {
        Form::Other
    }
}

fn is_directive(head: &str, compiled: &Compiled) -> bool {
    (compiled.directive.as_ref()).is_some_and(|directive| directive.is_match(head))
}

// The first branch that matches wins: suppression, directive, banner, doc rules, block run.
fn comment_finding(comment: &Comment<'_>, compiled: &Compiled) -> Option<Found> {
    let head = comment.head;
    let keyed = |rule, message| Found {
        line: comment.line,
        rule,
        key: head.to_owned(),
        message,
    };
    if let Some(Suppression::Comment(pattern)) = &compiled.suppression
        && pattern.is_match(head)
    {
        return Some(keyed(SUPPRESS, messages::COMMENT_SUPPRESSION.render(head)));
    }
    if is_directive(head, compiled) {
        return None;
    }
    if compiled.banner.is_match(head) {
        return Some(keyed(BANNER, messages::COMMENT_BANNER.to_owned()));
    }
    match comment.form {
        Form::Doc => jsdoc_finding(comment, compiled.docs.as_ref()),
        Form::Block => block_run(comment),
        Form::Line | Form::Other => None,
    }
}

fn block_run(comment: &Comment<'_>) -> Option<Found> {
    let span = comment.text.matches('\n').count() + 1;
    (span > MAX_COMMENT_LINES).then(|| Found {
        line: comment.line,
        rule: RUN,
        key: comment.text.to_owned(),
        message: messages::COMMENT_RUN.render(span),
    })
}

fn jsdoc_finding(comment: &Comment<'_>, docs: Option<&Docs>) -> Option<Found> {
    let Some(Docs::Jsdoc { tags, aliases }) = docs else {
        return None;
    };
    let named = |tag: &str| {
        let alias = aliases.iter().find(|(spelling, _)| *spelling == tag);
        alias.map_or(tag, |(_, name)| *name).to_owned()
    };
    let found: BTreeSet<String> = tags
        .captures_iter(comment.text)
        .filter_map(|captures| captures.get(1))
        .map(|tag| named(tag.as_str()))
        .collect();
    if found.is_empty() {
        return None;
    }
    let listed: Vec<String> = found.iter().map(|tag| format!("@{tag}")).collect();
    Some(Found {
        line: comment.line,
        rule: SECTION,
        key: found.into_iter().collect::<Vec<_>>().join("\n"),
        message: messages::COMMENT_JSDOC_TAGS.render(&listed.join(", ")),
    })
}

// Own-line line comments on consecutive lines. A directive, a banner, or a comment after code
// ends a run and joins none.
fn line_runs(comments: &[Comment<'_>], compiled: &Compiled) -> Vec<Found> {
    let mut runs: Vec<Vec<&Comment<'_>>> = Vec::new();
    let mut current: Vec<&Comment<'_>> = Vec::new();
    for comment in comments.iter().filter(|comment| comment.form == Form::Line) {
        let head = comment.head;
        let exempt = is_directive(head, compiled) || compiled.banner.is_match(head);
        let counts = comment.own_line && !exempt;
        let follows = current
            .last()
            .is_some_and(|last| comment.line == last.line + 1);
        if counts && follows {
            current.push(comment);
            continue;
        }
        let ended = std::mem::take(&mut current);
        if ended.len() > MAX_COMMENT_LINES {
            runs.push(ended);
        }
        if counts {
            current.push(comment);
        }
    }
    if current.len() > MAX_COMMENT_LINES {
        runs.push(current);
    }
    runs.into_iter().map(|run| run_finding(&run)).collect()
}

fn run_finding(run: &[&Comment<'_>]) -> Found {
    let texts: Vec<&str> = run.iter().map(|comment| comment.text).collect();
    Found {
        line: run[0].line,
        rule: RUN,
        key: texts.join("\n"),
        message: messages::COMMENT_RUN.render(run.len()),
    }
}

// A tree with a syntax error gives no docstring finding, as the reference's parse fails there.
fn docstring_findings(
    parsed: &Parsed,
    kinds: &DocstringKinds,
    private: &Regex,
    sections: &Regex,
) -> Vec<Found> {
    if parsed.has_error(parsed.root()) {
        return Vec::new();
    }
    let functions = parsed
        .descendants(parsed.root())
        .filter(|node| kinds.functions.contains(parsed.kind(*node)));
    let mut found = Vec::new();
    for function in functions {
        let Some(doc) = docstring(parsed, kinds, function) else {
            continue;
        };
        let Some(name) = field_text(parsed, function, kinds.name) else {
            continue;
        };
        let line = parsed.start_point(function).row + 1;
        let (rule, message) = if private.is_match(name) {
            (
                PRIVATE_DOC,
                messages::COMMENT_PRIVATE_DOCSTRING.render(name),
            )
        } else if sections.is_match(&doc) {
            (SECTION, messages::COMMENT_DOCSTRING_SECTION.render(name))
        } else {
            continue;
        };
        let key = name.to_owned();
        found.push(Found {
            line,
            rule,
            key,
            message,
        });
    }
    found
}

fn field_text(parsed: &Parsed, node: NodeId, field: Option<FieldId>) -> Option<&str> {
    let child = parsed.children_by_field(node, field?).next()?;
    Some(parsed.text(child))
}

// The text of a function's docstring: its body's first statement when that statement is one
// plain string, or plain strings that join into one. The grammar puts the string in the body.
fn docstring(parsed: &Parsed, kinds: &DocstringKinds, function: NodeId) -> Option<String> {
    let body = parsed.children_by_field(function, kinds.body?).next()?;
    let not_comment = |node: &NodeId| !kinds.comments.contains(parsed.kind(*node));
    let first = parsed.named_children(body).find(not_comment)?;
    if kinds.strings.contains(parsed.kind(first)) {
        return plain_string(parsed, first).map(str::to_owned);
    }
    if !kinds.joined.contains(parsed.kind(first)) {
        return None;
    }
    let strings = parsed.named_children(first).filter(not_comment);
    strings
        .map(|part| plain_string(parsed, part))
        .collect::<Option<Vec<&str>>>()
        .map(|parts| parts.concat())
}

// The text between the quotes of a string with no bytes, format or template prefix.
fn plain_string(parsed: &Parsed, string: NodeId) -> Option<&str> {
    let start = parsed.children(string).next()?;
    let end = parsed.children(string).last()?;
    let prefix = parsed.text(start);
    let mut letters = prefix.chars().take_while(char::is_ascii_alphabetic);
    if letters.any(|letter| !matches!(letter, 'r' | 'R' | 'u' | 'U')) {
        return None;
    }
    let range = parsed.end_byte(start) as usize..parsed.start_byte(end) as usize;
    parsed.source().get(range)
}

#[cfg(test)]
mod tests;
