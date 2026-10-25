use std::cell::{Cell, RefCell};

use syntax::{FieldCursor, FieldId, KindSet, Language, NodeId, Parsed};

use crate::slices::line_prefix;
use crate::spelling::{rstrip, truncate};

const SENTINEL: &str = "...";
const DOCSTRING_MAX_CHARS: usize = 200;
const DOCSTRING_PARENTS: [&str; 4] = ["module", "block", "class_body", "program"];
const DOC_COMMENT_MARKERS: [&str; 2] = ["outer_doc_comment_marker", "inner_doc_comment_marker"];
// The comment kinds that the language tables leave out: Kotlin's and Swift's block comments,
// JavaScript's `<!--` comments, and SQL's `/* */` comments.
const UNLISTED_COMMENTS: [&str; 3] = ["multiline_comment", "html_comment", "marginalia"];
// The doc comment forms of JSDoc, Javadoc and C#, of Go, and of LDoc.
const SLASH_STARS: DocForm = DocForm::Block(BlockForm {
    opens: Marker::exclusive("/**"),
    closes: "*/",
});
const TRIPLE_SLASHES: DocForm = DocForm::Run(RunForm {
    opens: Marker::exclusive("///"),
    continues: Marker::exclusive("///"),
});
const GO_LINES: DocForm = DocForm::Run(RunForm {
    opens: Marker::plain("//"),
    continues: Marker::plain("//"),
});
const GO_BLOCK: DocForm = DocForm::Block(BlockForm {
    opens: Marker::plain("/*"),
    closes: "*/",
});
const LDOC_LINES: DocForm = DocForm::Run(RunForm {
    opens: Marker::plain("---"),
    continues: Marker::plain("--"),
});
const LDOC_BLOCK: DocForm = DocForm::Block(BlockForm {
    opens: Marker::plain("--[[--"),
    closes: "]]",
});

/// What a skeleton keeps beyond the signatures: docstrings cut to one line, and line numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkeletonOptions {
    pub docstrings: bool,
    pub number_lines: bool,
}

/// Return the skeleton of a parse: each function body becomes `...`, comments go, and docstrings
/// and doc comments go or shrink to one line; every other line stays as the source spells it.
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
    comments: &'a KindSet,
    body: Option<FieldId>,
    // The plan visits nodes in document order, so one cursor reads every body field cheaply.
    fields: RefCell<FieldCursor<'a>>,
    style: DocStyle,
    // The last walk over a comment group: its parent, its first and last comment, and its answer.
    // The plan visits a group in document order, so one walk serves each later comment of it.
    walk: Cell<Option<Walk>>,
}

// Which lines to drop and which to replace, by 1-based line. An elided body owns its lines, and
// nothing writes inside them afterwards.
struct Plan {
    dropped: Vec<bool>,
    replaced: Vec<Option<String>>,
    owned: Vec<bool>,
    // A line of a run of doc comment lines: the run's kind, and whether the run is settled: it
    // kept its text, or it documents no code.
    doc_runs: Vec<Option<(&'static str, bool)>>,
}

// A doc comment: its kind, its marker, a block comment's closing marker, the first line of its
// text, and whether it documents code. A Rust marker is spelled as the source spells it.
struct DocComment<'a> {
    kind: &'static str,
    marker: &'a str,
    closer: Option<&'static str>,
    text: String,
    // A line that goes on with a run takes the run's state instead.
    attached: bool,
}

// The doc comment forms of a language, and whether its directive lines, such as
// `//go:noinline`, hold no text. Rust's doc comments come from the grammar's markers instead.
struct DocStyle {
    forms: &'static [DocForm],
    directives: bool,
}

#[derive(Clone, Copy)]
enum DocForm {
    Run(RunForm),
    Block(BlockForm),
}

// A run opens with a line comment that starts with `opens`, and goes on over the line comments
// right below it that start with `continues`.
#[derive(Clone, Copy)]
struct RunForm {
    opens: Marker,
    continues: Marker,
}

// One comment from `opens` to `closes`.
#[derive(Clone, Copy)]
struct BlockForm {
    opens: Marker,
    closes: &'static str,
}

// A comment marker. An exclusive marker is not followed by its last character, so `/***` and
// `////` open no doc comment.
#[derive(Clone, Copy)]
struct Marker {
    text: &'static str,
    exclusive: bool,
}

// A walk from the first comment of a group past its siblings to the last one. Node ids count in
// document order, so the group's later comments lie between its first and last id.
#[derive(Clone, Copy)]
struct Walk {
    parent: Option<NodeId>,
    first: NodeId,
    last: NodeId,
    answer: bool,
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
            comments: &extraction.comments,
            body: parsed.language().field("body"),
            fields: RefCell::new(FieldCursor::new(parsed)),
            style: doc_style(parsed.language()),
            walk: Cell::new(None),
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
                format!("{head} {{ {SENTINEL} }}{}", self.closing_tail(body))
            } else {
                format!("{head} {SENTINEL}")
            };
            plan.elide(first, last, vec![(first, sentinel)]);
            return;
        }
        let indent = " ".repeat(column);
        if opens_brace {
            let tail = self.closing_tail(body);
            plan.elide(
                first,
                last,
                vec![(first, format!("{indent}{{ {SENTINEL} }}{tail}"))],
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

    // Not every kind whose name holds `comment`: SQL's `keyword_comment` and `comment_statement`
    // are code.
    fn is_comment(&self, node: NodeId) -> bool {
        self.comments.contains(self.parsed.kind(node))
            || UNLISTED_COMMENTS.contains(&self.parsed.kind_name(node))
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

    // A doc comment that documents code keeps its marker and the first line of its text. A line
    // of a run goes when an earlier line of the run kept the text.
    fn plan_doc_comment(&self, plan: &mut Plan, node: NodeId, indent: &str) {
        let first = self.rows(node).0;
        let previous = plan.doc_run_kind(first - 1);
        let Some(doc) = self.doc_comment(node, previous) else {
            return;
        };
        match doc.closer {
            Some(closer) if doc.attached && !doc.text.is_empty() => plan.replace(
                first,
                format!("{indent}{} {} {closer}", doc.marker, doc.text),
            ),
            Some(_) => {}
            None => {
                let kept = format!("{indent}{} {}", doc.marker, doc.text);
                plan.run_line(first, &doc, kept);
            }
        }
    }

    // `previous` is the kind of the run on the line before, if any.
    fn doc_comment(&self, node: NodeId, previous: Option<&'static str>) -> Option<DocComment<'a>> {
        self.marked_doc_comment(node).or_else(|| {
            self.style.forms.iter().find_map(|form| match *form {
                DocForm::Run(run) => self.run_doc(node, run, previous),
                DocForm::Block(block) => self.block_doc(node, block),
            })
        })
    }

    // A Rust doc comment, which the grammar marks with a marker child.
    fn marked_doc_comment(&self, node: NodeId) -> Option<DocComment<'a>> {
        let parsed = self.parsed;
        let marker = parsed
            .children(node)
            .find(|child| DOC_COMMENT_MARKERS.contains(&parsed.kind_name(*child)))?;
        let body = parsed
            .children(node)
            .find(|child| parsed.kind_name(*child) == "doc_comment")
            .map_or("", |child| parsed.text(child));
        let start = parsed.start_byte(node) as usize;
        let marker_text = &parsed.source()[start..parsed.end_byte(marker) as usize];
        Some(DocComment {
            kind: parsed.kind_name(marker),
            marker: marker_text,
            closer: marker_text.starts_with("/*").then_some("*/"),
            text: first_text_line(body),
            attached: true,
        })
    }

    // A line of a run goes on with the run on the line before when it starts with the run's
    // `continues`, else it opens a run.
    fn run_doc(
        &self,
        node: NodeId,
        run: RunForm,
        previous: Option<&'static str>,
    ) -> Option<DocComment<'a>> {
        let comment = self.parsed.text(node);
        let kind = run.opens.text;
        let continued = (previous == Some(kind))
            .then(|| run.continues.rest(comment))
            .flatten();
        let (rest, marker) = match continued {
            Some(rest) => (rest, run.continues),
            None => (run.opens.rest(comment)?, run.opens),
        };
        Some(DocComment {
            kind,
            marker: kind,
            closer: None,
            text: line_text(rest, marker.text, self.style.directives),
            attached: continued.is_some() || self.documents_code(node),
        })
    }

    fn block_doc(&self, node: NodeId, block: BlockForm) -> Option<DocComment<'a>> {
        let comment = self.parsed.text(node);
        let inner = block
            .opens
            .rest(comment.strip_suffix(block.closes).unwrap_or(comment))?;
        Some(DocComment {
            kind: block.opens.text,
            marker: block.opens.text,
            closer: Some(block.closes),
            text: first_prose_line(inner),
            attached: self.documents_code(node),
        })
    }

    // Whether a named node follows the comment, past the comments right below it, with no blank
    // line between: a comment above a blank line or above a closing `}` documents nothing.
    fn documents_code(&self, node: NodeId) -> bool {
        let parent = self.parsed.parent(node);
        if let Some(walk) = self.walk.get().filter(|walk| walk.holds(parent, node)) {
            return walk.answer;
        }
        let (last, answer) = self.walk_comments(node);
        self.walk.set(Some(Walk {
            parent,
            first: node,
            last,
            answer,
        }));
        answer
    }

    // The last comment of the group that the comment opens, and whether a named node follows it.
    fn walk_comments(&self, node: NodeId) -> (NodeId, bool) {
        let mut last = node;
        let mut end = self.comment_end(node).0;
        while let Some(sibling) = self.parsed.next_sibling(last) {
            if self.rows(sibling).0 > end + 1 {
                return (last, false);
            }
            if !self.is_comment(sibling) {
                return (last, self.parsed.is_named(sibling));
            }
            end = self.comment_end(sibling).0;
            last = sibling;
        }
        (last, false)
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

    // The code after a body on its last line, up to the end of the outermost node that ends on
    // that line: an impl's `}` or a call's `);` stays, and a comment after them does not.
    fn closing_tail(&self, body: NodeId) -> &'a str {
        let end = self.parsed.end_point(body);
        let mut close = end.column;
        let mut node = body;
        while let Some(parent) = self.parsed.parent(node) {
            let parent_end = self.parsed.end_point(parent);
            if parent_end.row != end.row {
                break;
            }
            close = parent_end.column;
            node = parent;
        }
        &self.lines[end.row as usize][end.column as usize..close as usize]
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

// The first line of a doc comment's text that holds a word, cut at its limit.
fn first_text_line(body: &str) -> String {
    let line = text_lines(body).find(|line| text::split_whitespace(line).next().is_some());
    one_line(line.unwrap_or_default())
}

// The first line of a doc comment's text that holds a word outside its markup tags.
fn first_prose_line(body: &str) -> String {
    one_line(
        text_lines(body)
            .find(|line| holds_prose(line))
            .unwrap_or_default(),
    )
}

// A doc comment's lines. A line after the first drops the `*` that opens a block comment's line.
fn text_lines(body: &str) -> impl Iterator<Item = &str> {
    body.split('\n').enumerate().map(|(index, line)| {
        let line = line.trim_start_matches(text::is_space);
        if index > 0 {
            line.strip_prefix('*').unwrap_or(line)
        } else {
            line
        }
    })
}

fn one_line(line: &str) -> String {
    let words: Vec<&str> = text::split_whitespace(line).collect();
    truncate(&words.join(" "), DOCSTRING_MAX_CHARS)
}

// Whether a line holds a word outside its markup tags: `<summary>` holds none, and an unclosed
// `<` is a word.
fn holds_prose(line: &str) -> bool {
    let mut rest = line;
    while let Some(open) = rest.find('<') {
        if !text::strip(&rest[..open]).is_empty() {
            return true;
        }
        let Some(close) = rest[open..].find('>') else {
            return true;
        };
        rest = &rest[open + close + 1..];
    }
    !text::strip(rest).is_empty()
}

// A run line's text after its marker. A directive holds none, and the repeats of the marker's
// last character go, so that a Lua line of hyphens holds none.
fn line_text(rest: &str, marker: &str, directives: bool) -> String {
    if directives && is_directive(rest) {
        return String::new();
    }
    first_prose_line(rest.trim_start_matches(|c: char| marker.ends_with(c)))
}

// A Go comment directive after its `//`, by the rule of go/ast: `line `, `extern `, `export `,
// or `[a-z0-9]+:[a-z0-9]`.
fn is_directive(rest: &str) -> bool {
    if ["line ", "extern ", "export "]
        .iter()
        .any(|prefix| rest.starts_with(prefix))
    {
        return true;
    }
    let word = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    let bytes = rest.as_bytes();
    rest.find(':').is_some_and(|colon| {
        colon > 0 && bytes[..colon].iter().all(word) && bytes.get(colon + 1).is_some_and(word)
    })
}

fn doc_style(language: Language) -> DocStyle {
    let (forms, directives): (&'static [DocForm], bool) = match language {
        Language::Javascript | Language::Typescript | Language::Tsx => (&[SLASH_STARS], false),
        Language::Java | Language::Csharp => (&[SLASH_STARS, TRIPLE_SLASHES], false),
        Language::Go => (&[GO_LINES, GO_BLOCK], true),
        Language::Lua => (&[LDOC_LINES, LDOC_BLOCK], false),
        _ => (&[], false),
    };
    DocStyle { forms, directives }
}

impl Marker {
    const fn exclusive(text: &'static str) -> Marker {
        Marker {
            text,
            exclusive: true,
        }
    }

    const fn plain(text: &'static str) -> Marker {
        Marker {
            text,
            exclusive: false,
        }
    }

    // The comment after the marker, none when the comment does not open with it.
    fn rest(self, comment: &str) -> Option<&str> {
        let rest = comment.strip_prefix(self.text)?;
        let repeated = rest.starts_with(|c: char| self.text.ends_with(c));
        (!(self.exclusive && repeated)).then_some(rest)
    }
}

impl Walk {
    // Whether the walk passed the comment: a later sibling of its first comment, up to its last.
    fn holds(self, parent: Option<NodeId>, node: NodeId) -> bool {
        self.parent == parent && self.first < node && node <= self.last
    }
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

    fn doc_run_kind(&self, line: usize) -> Option<&'static str> {
        self.doc_runs[line].map(|(kind, _)| kind)
    }

    // A line goes on with the run on the line before, else it opens one, and a run above no code
    // is settled at once. A run keeps its first line that holds text.
    fn run_line(&mut self, line: usize, doc: &DocComment, kept: String) {
        let settled = match self.doc_runs[line - 1] {
            Some((kind, settled)) if kind == doc.kind => settled,
            _ => !doc.attached,
        };
        let has_text = !doc.text.is_empty();
        self.doc_runs[line] = Some((doc.kind, settled || has_text));
        if has_text && !settled {
            self.replace(line, kept);
        }
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
