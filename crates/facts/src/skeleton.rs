use std::cell::RefCell;

use syntax::{FieldCursor, FieldId, KindSet, NodeId, Parsed};

use crate::slices::line_prefix;
use crate::spelling::{rstrip, truncate};

const SENTINEL: &str = "...";
const DOCSTRING_MAX_CHARS: usize = 200;
const DOCSTRING_PARENTS: [&str; 4] = ["module", "block", "class_body", "program"];
const DOC_COMMENT_MARKERS: [&str; 2] = ["outer_doc_comment_marker", "inner_doc_comment_marker"];

/// What a skeleton keeps beyond the signatures: docstrings cut to one line, and line numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkeletonOptions {
    pub docstrings: bool,
    pub number_lines: bool,
}

/// Return the skeleton of a parse: each function body becomes `...`, comments go, and docstrings
/// and Rust doc comments go or shrink to one line; every other line stays as the source spells it.
#[must_use]
pub fn skeletonize(parsed: &Parsed, options: SkeletonOptions) -> String {
    let lines: Vec<&str> = parsed.source().split('\n').collect();
    let skeleton = Skeleton::new(parsed, &lines);
    let mut plan = Plan::new(lines.len());
    for node in parsed.descendants(parsed.root()) {
        skeleton.plan_node(&mut plan, node, options.docstrings);
    }
    render(&lines, &plan, options.number_lines)
}

// The parse, its lines, and the kinds and field that the rules read.
struct Skeleton<'a> {
    parsed: &'a Parsed,
    lines: &'a [&'a str],
    functions: &'a KindSet,
    extra_functions: &'a KindSet,
    bodies: &'a KindSet,
    body: Option<FieldId>,
    // The plan visits nodes in document order, so one cursor reads every body field cheaply.
    fields: RefCell<FieldCursor<'a>>,
}

// Which lines to drop and which to replace, by 1-based line. An elided body owns its lines, and
// nothing writes inside them afterwards.
struct Plan {
    dropped: Vec<bool>,
    replaced: Vec<Option<String>>,
    owned: Vec<bool>,
    // A line of a `///` or `//!` run: its marker kind, and whether the run has kept its text.
    doc_runs: Vec<Option<(&'static str, bool)>>,
}

// A Rust doc comment: its marker kind, its marker as the source spells it, and the first line of
// its text.
struct DocComment<'a> {
    kind: &'static str,
    marker: &'a str,
    text: String,
}

impl<'a> Skeleton<'a> {
    fn new(parsed: &'a Parsed, lines: &'a [&'a str]) -> Skeleton<'a> {
        let extraction = parsed.language().extraction();
        Skeleton {
            parsed,
            lines,
            functions: &extraction.declarations.functions,
            extra_functions: &extraction.skeleton_functions,
            bodies: &extraction.bodies,
            body: parsed.language().field("body"),
            fields: RefCell::new(FieldCursor::new(parsed)),
        }
    }

    fn plan_node(&self, plan: &mut Plan, node: NodeId, keep_docstring: bool) {
        let kind = self.parsed.kind(node);
        if self.functions.contains(kind) || self.extra_functions.contains(kind) {
            self.plan_function(plan, node, keep_docstring);
        } else if self.is_comment(node) {
            self.plan_comment(plan, node, keep_docstring);
        } else if self.is_leading_docstring(node) {
            self.plan_docstring(plan, node, keep_docstring);
        }
    }

    // A body that opens on its signature's line keeps the signature and gets the sentinel there;
    // a brace on its own line keeps the braces around the sentinel.
    fn plan_function(&self, plan: &mut Plan, node: NodeId, keep_docstring: bool) {
        let Some(body) = self.body_of(node) else {
            return;
        };
        let (first, last) = self.rows(body);
        if plan.owns(first) {
            return;
        }
        let line = self.lines[first - 1];
        let head = &line[..self.parsed.start_point(body).column as usize];
        let opens_brace = line[head.len()..].starts_with('{');
        let column = head.chars().count();
        if column > line.chars().count() - line.trim_start_matches(text::is_space).chars().count() {
            let head = rstrip(head);
            let sentinel = if opens_brace {
                format!("{head} {{ {SENTINEL} }}")
            } else {
                format!("{head} {SENTINEL}")
            };
            plan.elide(first, last, vec![(first, sentinel)]);
            return;
        }
        let indent = " ".repeat(column);
        if opens_brace {
            plan.elide(
                first,
                last,
                vec![(first, format!("{indent}{{ {SENTINEL} }}"))],
            );
            return;
        }
        let kept = match self.body_docstring(body).filter(|_| keep_docstring) {
            Some(docstring) => self.kept_docstring(docstring, &indent, last),
            None => vec![(first, format!("{indent}{SENTINEL}"))],
        };
        plan.elide(first, last, kept);
    }

    // The docstring's line, and the sentinel on the body's last line when the body goes on.
    fn kept_docstring(&self, docstring: NodeId, indent: &str, last: usize) -> Vec<(usize, String)> {
        let (doc_first, doc_last) = self.rows(docstring);
        let mut kept = vec![(
            doc_first,
            format!("{indent}{}", self.one_line_docstring(docstring)),
        )];
        if last > doc_last {
            kept.push((last, format!("{indent}{SENTINEL}")));
        }
        kept
    }

    // The kind name decides, not the language's comment kinds: kinds that table leaves out, such
    // as `multiline_comment` and `html_comment`, go too.
    fn is_comment(&self, node: NodeId) -> bool {
        self.parsed.kind_name(node).contains("comment")
    }

    // A comment goes; the code that shares its first or last line stays, without the space it
    // leaves. A node inside a comment, such as Rust's `doc_comment`, is a part of that comment.
    fn plan_comment(&self, plan: &mut Plan, node: NodeId, keep_docstring: bool) {
        if self
            .parsed
            .parent(node)
            .is_some_and(|parent| self.is_comment(parent))
        {
            return;
        }
        let first = self.rows(node).0;
        let (last, tail) = self.comment_end(node);
        let head = &self.lines[first - 1][..self.parsed.start_point(node).column as usize];
        let (head_blank, tail_blank) = (text::strip(head).is_empty(), text::strip(tail).is_empty());
        if head_blank && tail_blank {
            plan.drop_range(first, last);
            if keep_docstring {
                self.plan_doc_comment(plan, node, head);
            }
            return;
        }
        plan.drop_range(first + 1, last);
        let kept = if head_blank {
            format!("{head}{}", tail.trim_start_matches(text::is_space))
        } else {
            format!("{}{tail}", rstrip(head))
        };
        plan.replace(first, rstrip(&kept).to_owned());
    }

    // The comment's last line and the text after the comment there. A token that holds its
    // closing newline ends at column 0 of the next row, so it ends on the row before.
    fn comment_end(&self, node: NodeId) -> (usize, &'a str) {
        let (first, last) = self.rows(node);
        let column = self.parsed.end_point(node).column as usize;
        if column == 0 && last > first {
            return (last - 1, "");
        }
        (last, &self.lines[last - 1][column..])
    }

    // A Rust doc comment keeps its marker and the first line of its text. A line of a `///` or
    // `//!` run goes when an earlier line of the run kept the text.
    fn plan_doc_comment(&self, plan: &mut Plan, node: NodeId, indent: &str) {
        let Some(doc) = self.doc_comment(node) else {
            return;
        };
        let first = self.rows(node).0;
        let has_text = !doc.text.is_empty();
        if doc.marker.starts_with("/*") {
            if has_text {
                plan.replace(first, format!("{indent}{} {} */", doc.marker, doc.text));
            }
            return;
        }
        let run_has_text = plan.doc_run_has_text(first - 1, doc.kind);
        plan.extend_doc_run(first, doc.kind, run_has_text || has_text);
        if has_text && !run_has_text {
            plan.replace(first, format!("{indent}{} {}", doc.marker, doc.text));
        }
    }

    fn doc_comment(&self, node: NodeId) -> Option<DocComment<'a>> {
        let parsed = self.parsed;
        let marker = parsed
            .children(node)
            .find(|child| DOC_COMMENT_MARKERS.contains(&parsed.kind_name(*child)))?;
        let body = parsed
            .children(node)
            .find(|child| parsed.kind_name(*child) == "doc_comment")
            .map_or("", |child| parsed.text(child));
        let start = parsed.start_byte(node) as usize;
        Some(DocComment {
            kind: parsed.kind_name(marker),
            marker: &parsed.source()[start..parsed.end_byte(marker) as usize],
            text: first_text_line(body),
        })
    }

    fn plan_docstring(&self, plan: &mut Plan, node: NodeId, keep: bool) {
        let (first, last) = self.rows(node);
        plan.drop_range(first, last);
        if keep {
            let head = &self.lines[first - 1][..self.parsed.start_point(node).column as usize];
            let indent = " ".repeat(head.chars().count());
            plan.replace(first, format!("{indent}{}", self.one_line_docstring(node)));
        }
    }

    // The body field when it is a block, none when the field holds something else, else the last
    // child that is a block.
    fn body_of(&self, node: NodeId) -> Option<NodeId> {
        let field = self
            .body
            .and_then(|body| self.fields.borrow_mut().child_by_field(node, body));
        if let Some(body) = field {
            return self.bodies.contains(self.parsed.kind(body)).then_some(body);
        }
        let children: Vec<NodeId> = self.parsed.children(node).collect();
        children
            .into_iter()
            .rev()
            .find(|child| self.bodies.contains(self.parsed.kind(*child)))
    }

    // A string statement that opens a module, a class body or a program.
    fn is_leading_docstring(&self, node: NodeId) -> bool {
        if !self.is_string_statement(node) {
            return false;
        }
        let Some(parent) = self.parsed.parent(node) else {
            return false;
        };
        let parent_kind = self.parsed.kind_name(parent);
        if !DOCSTRING_PARENTS.contains(&parent_kind) {
            return false;
        }
        let in_class = self
            .parsed
            .parent(parent)
            .is_some_and(|owner| self.parsed.kind_name(owner).contains("class"));
        if parent_kind == "block" && !in_class {
            return false;
        }
        self.parsed.named_children(parent).next() == Some(node)
    }

    fn body_docstring(&self, body: NodeId) -> Option<NodeId> {
        let first = self.parsed.named_children(body).next()?;
        self.is_string_statement(first).then_some(first)
    }

    // A bare string, or an expression statement that holds one first.
    fn is_string_statement(&self, node: NodeId) -> bool {
        let kind = self.parsed.kind_name(node);
        kind == "string"
            || (kind == "expression_statement"
                && self
                    .parsed
                    .children(node)
                    .next()
                    .is_some_and(|first| self.parsed.kind_name(first) == "string"))
    }

    // The docstring on one line, cut at its limit, with its prefix and its quotes.
    fn one_line_docstring(&self, node: NodeId) -> String {
        let text = text::strip(self.parsed.text(node));
        let prefix = string_prefix(text);
        let inner = &text[prefix.len()..];
        let quote = quote_of(inner);
        let quoted = !quote.is_empty()
            && inner.starts_with(quote)
            && inner.ends_with(quote)
            && inner.chars().count() >= 2 * quote.chars().count();
        let inner = if quoted {
            &inner[quote.len()..inner.len() - quote.len()]
        } else {
            inner
        };
        let words: Vec<&str> = text::split_whitespace(inner).collect();
        let flattened = truncate(&words.join(" "), DOCSTRING_MAX_CHARS);
        format!("{prefix}{quote}{flattened}{quote}")
    }

    fn rows(&self, node: NodeId) -> (usize, usize) {
        let first = self.parsed.start_point(node).row as usize + 1;
        let last = self.parsed.end_point(node).row as usize + 1;
        (first, last)
    }
}

// A string prefix: at most three ASCII letters right before the first quote, else empty.
fn string_prefix(text: &str) -> &str {
    let letters = text.bytes().take_while(u8::is_ascii_alphabetic).count();
    let quoted = text[letters..].starts_with(['\'', '"']);
    if letters <= 3 && quoted {
        &text[..letters]
    } else {
        ""
    }
}

// The first line of a doc comment's text that holds a word, cut at its limit. A line after the
// first drops the `*` that opens a block comment's line.
fn first_text_line(body: &str) -> String {
    let words = body
        .split('\n')
        .enumerate()
        .map(|(index, line)| {
            let line = line.trim_start_matches(text::is_space);
            let line = if index > 0 {
                line.strip_prefix('*').unwrap_or(line)
            } else {
                line
            };
            text::split_whitespace(line).collect::<Vec<&str>>()
        })
        .find(|words| !words.is_empty())
        .unwrap_or_default();
    truncate(&words.join(" "), DOCSTRING_MAX_CHARS)
}

// A triple quote anywhere in the text wins, else the text's first character.
fn quote_of(inner: &str) -> &str {
    if inner.contains("\"\"\"") {
        "\"\"\""
    } else if inner.contains("'''") {
        "'''"
    } else {
        inner
            .chars()
            .next()
            .map_or("", |first| &inner[..first.len_utf8()])
    }
}

impl Plan {
    fn new(lines: usize) -> Plan {
        Plan {
            dropped: vec![false; lines + 1],
            replaced: vec![None; lines + 1],
            owned: vec![false; lines + 1],
            doc_runs: vec![None; lines + 1],
        }
    }

    fn owns(&self, line: usize) -> bool {
        self.owned[line]
    }

    fn doc_run_has_text(&self, line: usize, kind: &str) -> bool {
        self.doc_runs[line].is_some_and(|(run, has_text)| run == kind && has_text)
    }

    fn extend_doc_run(&mut self, line: usize, kind: &'static str, has_text: bool) {
        self.doc_runs[line] = Some((kind, has_text));
    }

    fn drop_range(&mut self, first: usize, last: usize) {
        for line in first..=last {
            self.dropped[line] = true;
        }
    }

    fn replace(&mut self, line: usize, text: String) {
        if self.owned[line] {
            return;
        }
        self.dropped[line] = false;
        self.replaced[line] = Some(text);
    }

    // Drop the body's lines, write what it keeps, and claim the range: one decision, so that no
    // later rule finds the range dropped but unclaimed.
    fn elide(&mut self, first: usize, last: usize, kept: Vec<(usize, String)>) {
        self.drop_range(first, last);
        for (line, text) in kept {
            self.dropped[line] = false;
            self.replaced[line] = Some(text);
        }
        for line in first..=last {
            self.owned[line] = true;
        }
    }
}

// The lines that survive, with runs of blank lines collapsed to one, no leading blank, and no
// trailing blank.
fn render(lines: &[&str], plan: &Plan, number_lines: bool) -> String {
    let mut rendered: Vec<String> = Vec::new();
    let mut previous_blank = true;
    let mut content_end = 0;
    for (index, original) in lines.iter().enumerate() {
        let number = index + 1;
        if plan.dropped[number] {
            continue;
        }
        let line = plan.replaced[number].as_deref().unwrap_or(original);
        let prefix = if number_lines {
            line_prefix(u32::try_from(number).unwrap_or(u32::MAX))
        } else {
            String::new()
        };
        if text::strip(line).is_empty() {
            if !previous_blank {
                previous_blank = true;
                rendered.push(prefix);
            }
            continue;
        }
        previous_blank = false;
        rendered.push(prefix + line);
        content_end = rendered.len();
    }
    rendered.truncate(content_end);
    if rendered.is_empty() {
        String::new()
    } else {
        rendered.join("\n") + "\n"
    }
}
