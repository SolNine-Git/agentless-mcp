//! The Rust doc and attribute rules of `comment_delta`. They read the source with every comment and
//! string literal of the tree blanked, so a delimiter or a `#[` inside one never counts.

use prompts::messages;
use regex::Regex;
use syntax::{LiteralClass, Parsed};

use super::{Comment, Found, Marker, PRIVATE_DOC, SECTION, SUPPRESS, regex};

// The heads that decide the kind of a `{` block. A trait or an enum head gives its visibility.
const FUNCTION: &str = r"\bfn\b";
const TRAIT: &str = r"(?:^|[\s\]])(?:(?P<visibility>pub\b(?:\s*\([^)]*\))?)\s*)?(?:unsafe\s+)?(?:auto\s+)?trait\s+\w";
const ENUM: &str = r"(?:^|[\s\]])(?:(?P<visibility>pub\b(?:\s*\([^)]*\))?)\s*)?enum\s+\w";
const IMPL: &str = r"\bimpl\b";
const FOR: &str = r"\bfor\b\s*(?:[^<\s]|$)";
const ATTRIBUTE: &str = r"#!?\[";

// The Rust doc rules, compiled.
pub(super) struct RustItems {
    item: Regex,
    public: &'static str,
    export: &'static str,
    facing: Regex,
    sections: Regex,
    heads: Heads,
}

struct Heads {
    function: Regex,
    trait_head: Regex,
    enum_head: Regex,
    impl_head: Regex,
    trait_impl: Regex,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Other,
    Trait,
    Enum,
    TraitImpl,
    Macro,
    Group,
    Facing,
}

impl Scope {
    // Macro bodies, groups and the scopes whose docs callers read at run time.
    fn unchecked(self) -> bool {
        matches!(self, Self::Macro | Self::Group | Self::Facing)
    }
}

#[derive(Clone, Copy)]
struct Frame {
    scope: Scope,
    public: bool,
    start: usize,
}

// The head of the item after a doc comment, and the attributes between them.
struct Item<'c> {
    visibility: Option<&'c str>,
    kind: Option<&'c str>,
    name: &'c str,
    attributes: Vec<&'c str>,
}

impl RustItems {
    pub(super) fn new(
        item: &str,
        public: &'static str,
        export: &'static str,
        facing: &str,
        sections: &str,
    ) -> Self {
        RustItems {
            item: regex(item),
            public,
            export,
            facing: regex(facing),
            sections: regex(sections),
            heads: Heads {
                function: regex(FUNCTION),
                trait_head: regex(TRAIT),
                enum_head: regex(ENUM),
                impl_head: regex(IMPL),
                trait_impl: regex(FOR),
            },
        }
    }

    pub(super) fn findings(&self, comments: &[Comment<'_>], code: &str) -> Vec<Found> {
        let runs = doc_runs(comments);
        let starts: Vec<usize> = runs.iter().map(|run| run[0].start).collect();
        let scopes = self.scopes(code, &starts);
        let mut found = Vec::new();
        for (run, (scope, public)) in runs.iter().zip(scopes) {
            let last = run.last().expect("a run holds a comment");
            let item = self.item_after(code, last.stop);
            let keyed = |rule, message| Found {
                line: run[0].line,
                rule,
                key: item.name.to_owned(),
                message,
            };
            if let Some(message) = self.issue(scope, public, &item) {
                found.push(keyed(PRIVATE_DOC, message));
            } else if !scope.unchecked() && self.sections.is_match(&doc_text(run)) {
                found.push(keyed(
                    SECTION,
                    messages::COMMENT_DOC_SECTION.render(item.name),
                ));
            }
        }
        found
    }

    fn issue(&self, scope: Scope, public: bool, item: &Item<'_>) -> Option<String> {
        if self.unchecked(scope, item) {
            return None;
        }
        if scope == Scope::TraitImpl {
            return Some(messages::COMMENT_TRAIT_IMPL_DOC.render(item.name));
        }
        let visible = if matches!(scope, Scope::Trait | Scope::Enum) {
            public
        } else if item.kind == Some("macro_rules!") {
            item.attributes
                .iter()
                .any(|attribute| attribute.contains(self.export))
        } else {
            item.visibility == Some(self.public)
        };
        (!visible).then(|| messages::COMMENT_PRIVATE_ITEM_DOC.render(item.name))
    }

    fn unchecked(&self, scope: Scope, item: &Item<'_>) -> bool {
        let unnamed = item.name.is_empty() || item.name.starts_with('$');
        let facing = item
            .attributes
            .iter()
            .any(|attribute| self.facing.is_match(attribute));
        scope.unchecked() || item.kind == Some("impl") || unnamed || facing
    }

    // The scope at each offset, by a walk over the delimiters. `offsets` is in ascending order.
    fn scopes(&self, code: &str, offsets: &[usize]) -> Vec<(Scope, bool)> {
        let mut frames = vec![Frame {
            scope: Scope::Other,
            public: false,
            start: 0,
        }];
        let mut scopes = Vec::with_capacity(offsets.len());
        let structure = code
            .bytes()
            .enumerate()
            .filter(|(_, byte)| b"{}()[];".contains(byte));
        for (at, byte) in structure {
            while scopes.len() < offsets.len() && offsets[scopes.len()] < at {
                scopes.push(top(&frames));
            }
            self.step(&mut frames, code, at, byte);
        }
        scopes.resize(offsets.len(), top(&frames));
        scopes
    }

    fn step(&self, frames: &mut Vec<Frame>, code: &str, at: usize, byte: u8) {
        let current = *frames.last().expect("the root frame stays");
        match byte {
            b'(' | b'[' => {
                let scope = if follows_bang(code, at) {
                    Scope::Macro
                } else {
                    Scope::Group
                };
                frames.push(opened(scope, false, at));
            }
            b')' | b']' => {
                if frames.len() > 1 && matches!(current.scope, Scope::Group | Scope::Macro) {
                    frames.pop();
                }
            }
            b'{' => {
                let (scope, public) = self.block(&code[current.start..at], current);
                frames.push(opened(scope, public, at));
            }
            b'}' => close_block(frames, at),
            _ => frames.last_mut().expect("the root frame stays").start = at + 1,
        }
    }

    // A block inside a macro body, an enum or a facing scope takes its parent's scope.
    fn block(&self, header: &str, parent: Frame) -> (Scope, bool) {
        if matches!(parent.scope, Scope::Macro | Scope::Enum | Scope::Facing) {
            return (parent.scope, parent.public);
        }
        let head = text::split_whitespace(header).collect::<Vec<_>>().join(" ");
        if head.ends_with('!') {
            return (Scope::Macro, false);
        }
        if self.facing.is_match(&head) {
            return (Scope::Facing, false);
        }
        self.item_kind(&head)
    }

    fn item_kind(&self, head: &str) -> (Scope, bool) {
        let heads = &self.heads;
        if heads.function.is_match(head) {
            return (Scope::Other, false);
        }
        let named = [
            (Scope::Trait, &heads.trait_head),
            (Scope::Enum, &heads.enum_head),
        ];
        for (scope, pattern) in named {
            if let Some(captures) = pattern.captures(head) {
                let visibility = captures.name("visibility").map(|found| found.as_str());
                return (scope, visibility == Some(self.public));
            }
        }
        if heads.impl_head.is_match(head) && heads.trait_impl.is_match(head) {
            return (Scope::TraitImpl, false);
        }
        (Scope::Other, false)
    }

    fn item_after<'c>(&self, code: &'c str, at: usize) -> Item<'c> {
        let mut at = skip_space(code, at);
        let mut attributes = Vec::new();
        while code[at..].starts_with("#[") {
            let end = bracket_end(code, at + 1);
            attributes.push(&code[at..end]);
            at = skip_space(code, end);
        }
        let captures = self.item.captures(&code[at..]);
        let group = |name| {
            let found = captures.as_ref().and_then(|captures| captures.name(name));
            found.map(|found| found.as_str())
        };
        Item {
            visibility: group("visibility"),
            kind: group("kind"),
            name: group("name").unwrap_or(""),
            attributes,
        }
    }
}

const fn opened(scope: Scope, public: bool, at: usize) -> Frame {
    Frame {
        scope,
        public,
        start: at + 1,
    }
}

// A `}` closes the groups that a missing `)` or `]` left open, then the block itself.
fn close_block(frames: &mut Vec<Frame>, at: usize) {
    while frames.len() > 1 {
        let popped = frames.pop().expect("more than the root frame");
        if popped.scope != Scope::Group {
            break;
        }
    }
    frames.last_mut().expect("the root frame stays").start = at + 1;
}

fn top(frames: &[Frame]) -> (Scope, bool) {
    let frame = frames.last().expect("the root frame stays");
    (frame.scope, frame.public)
}

// Outer doc comments: `///` comments on consecutive lines join one run, and each `/** */` is one.
fn doc_runs<'a, 't>(comments: &'a [Comment<'t>]) -> Vec<Vec<&'a Comment<'t>>> {
    let mut runs: Vec<Vec<&Comment<'_>>> = Vec::new();
    let docs = comments
        .iter()
        .filter(|comment| comment.marker == Some(Marker::Outer));
    for comment in docs {
        let joins = runs.last().and_then(|run| run.last()).is_some_and(|last| {
            let both = comment.text.starts_with("///") && last.text.starts_with("///");
            both && comment.line == last.end + 1
        });
        match runs.last_mut() {
            Some(run) if joins => run.push(comment),
            _ => runs.push(vec![comment]),
        }
    }
    runs
}

fn doc_text(run: &[&Comment<'_>]) -> String {
    if run[0].text.starts_with("///") {
        let lines: Vec<&str> = run.iter().map(|comment| &comment.text[3..]).collect();
        return lines.join("\n");
    }
    let text = run[0].text;
    let body = text.get(3..text.len().saturating_sub(2)).unwrap_or("");
    let lines: Vec<&str> = text::splitlines(body, false)
        .into_iter()
        .map(|line| {
            let line = text::strip(line);
            line.strip_prefix('*').unwrap_or(line)
        })
        .collect();
    lines.join("\n")
}

fn follows_bang(code: &str, at: usize) -> bool {
    code[..at].trim_end_matches(text::is_space).ends_with('!')
}

fn skip_space(code: &str, at: usize) -> usize {
    let rest = &code[at..];
    at + (rest.len() - rest.trim_start_matches(text::is_space).len())
}

// The offset after the `]` that closes the `[` at `at`, or the end of the code.
fn bracket_end(code: &str, at: usize) -> usize {
    let mut depth = 0_i64;
    for (offset, byte) in code.bytes().enumerate().skip(at) {
        depth += match byte {
            b'[' => 1,
            b']' => -1,
            _ => 0,
        };
        if depth == 0 {
            return offset + 1;
        }
    }
    code.len()
}

// The source with each comment and string literal blanked and its newlines kept, so every offset
// stays the same.
pub(super) fn code(parsed: &Parsed) -> String {
    let source = parsed.source();
    let Some(spec) = parsed.language().spec() else {
        return source.to_owned();
    };
    let blanked = |kind| {
        spec.comments.contains(kind) || spec.clones.literal_class(kind) == Some(LiteralClass::Str)
    };
    let mut code = String::with_capacity(source.len());
    let mut last = 0;
    for node in parsed.descendants(parsed.root()) {
        let range = parsed.byte_range(node);
        if range.start < last || !blanked(parsed.kind(node)) {
            continue;
        }
        code.push_str(&source[last..range.start]);
        let bytes = source[range.clone()].bytes();
        code.extend(bytes.map(|byte| if byte == b'\n' { '\n' } else { ' ' }));
        last = range.end;
    }
    code.push_str(&source[last..]);
    code
}

// Each attribute whose body, with whitespace collapsed, the pattern matches.
pub(super) fn attribute_findings(source: &str, code: &str, pattern: &Regex) -> Vec<Found> {
    let newlines: Vec<usize> = code.match_indices('\n').map(|(at, _)| at).collect();
    let mut found = Vec::new();
    for opener in regex(ATTRIBUTE).find_iter(code) {
        let end = bracket_end(code, opener.end() - 1);
        let inside = source.get(opener.end()..last_char_start(source, end));
        let body = text::split_whitespace(inside.unwrap_or("")).collect::<Vec<_>>();
        let body = body.join(" ");
        if !pattern.is_match(&body) {
            continue;
        }
        let head = format!("{}{body}]", opener.as_str());
        let line = newlines.partition_point(|at| *at < opener.start()) + 1;
        found.push(Found {
            line: u32::try_from(line).unwrap_or(u32::MAX),
            rule: SUPPRESS,
            message: messages::COMMENT_SUPPRESSION.render(&head),
            key: head,
        });
    }
    found
}

// The start of the last character before `end`: the `]` of a closed attribute, or the last
// character of a source whose attribute never closes.
fn last_char_start(source: &str, end: usize) -> usize {
    source[..end]
        .char_indices()
        .next_back()
        .map_or(0, |(at, _)| at)
}
