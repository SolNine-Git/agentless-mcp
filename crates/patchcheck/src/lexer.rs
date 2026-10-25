//! The call-site scan and the bracket-depth scanner that the arity and reference checks read. Every
//! index counts code points, so a text is a slice of chars.

use std::collections::BTreeSet;

use facts::{Ref, Symbol};

const QUOTES: [char; 2] = ['"', '\''];
const OPENING: [char; 3] = ['(', '[', '{'];
const CLOSING: [char; 3] = [')', ']', '}'];
const COMMENT: char = '#';
const STRING_PREFIXES: [char; 8] = ['r', 'R', 'b', 'B', 'u', 'U', 'f', 'F'];
const LAMBDA: [char; 6] = ['l', 'a', 'm', 'b', 'd', 'a'];
const POSITIONAL_ONLY: [char; 1] = ['/'];
const TRIPLE: usize = 3;

// The depth of a character inside a string literal or a comment. An unbalanced closing bracket
// also takes the text below depth 0, so -1 can mean either, as in the reference scanner.
const INSIDE_STRING: i64 = -1;

// One call that a replacement block makes: the callee, its 1-based line in the block, and the
// index of its opening parenthesis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CallSite {
    pub(crate) name: String,
    pub(crate) line: u32,
    pub(crate) offset: usize,
}

// How many positional arguments a captured signature accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Shape {
    pub(crate) required: usize,
    pub(crate) total: usize,
}

// The calls `name(` of `text` whose name the grammar reported as a reference on that line, minus
// the declarations of the block.
pub(crate) fn call_sites(text: &[char], refs: &[Ref], introduced: &[Symbol]) -> Vec<CallSite> {
    let coded: BTreeSet<(&str, u32)> = refs
        .iter()
        .filter(|reference| reference.is_reference())
        .map(|reference| (reference.name.as_str(), reference.line))
        .collect();
    let declared: BTreeSet<(&str, u32)> = introduced
        .iter()
        .map(|symbol| (symbol.name.as_str(), symbol.name_line))
        .collect();
    let starts = line_starts(text);
    let mut sites = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let Some((name_end, end)) = call_at(text, at) else {
            at += 1;
            continue;
        };
        let name: String = text[at..name_end].iter().collect();
        let line = line_of(&starts, at);
        let key = (name.as_str(), line);
        if coded.contains(&key) && !declared.contains(&key) {
            sites.push(CallSite {
                name,
                line,
                offset: end - 1,
            });
        }
        at = end;
    }
    sites
}

// The pattern `(?<![\w.])([A-Za-z_][A-Za-z0-9_]*)\s*\(` at `at`: the end of the name and of the
// match.
fn call_at(text: &[char], at: usize) -> Option<(usize, usize)> {
    let first = text[at];
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    if at > 0 && (text::is_word(text[at - 1]) || text[at - 1] == '.') {
        return None;
    }
    let name_end = (at + 1..text.len())
        .find(|&index| !(text[index].is_ascii_alphanumeric() || text[index] == '_'))
        .unwrap_or(text.len());
    let paren = (name_end..text.len())
        .find(|&index| !text::is_space(text[index]))
        .filter(|&index| text[index] == '(')?;
    Some((name_end, paren + 1))
}

fn line_starts(text: &[char]) -> Vec<usize> {
    let breaks = text
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == '\n')
        .map(|(index, _)| index + 1);
    std::iter::once(0).chain(breaks).collect()
}

fn line_of(starts: &[usize], offset: usize) -> u32 {
    let line = starts.partition_point(|&start| start <= offset).max(1);
    u32::try_from(line).unwrap_or(u32::MAX)
}

// The bracket depth of each character: an opening bracket carries the depth inside it and a
// closing one the depth it closes; a string or comment character is INSIDE_STRING.
pub(crate) fn structure(text: &[char]) -> Vec<i64> {
    let mut depths = vec![0; text.len()];
    let mut depth = 0;
    let mut index = 0;
    while index < text.len() {
        let c = text[index];
        if QUOTES.contains(&c) || c == COMMENT {
            let end = literal_end(text, index);
            depths[index..end].fill(INSIDE_STRING);
            index = end;
            continue;
        }
        if OPENING.contains(&c) {
            depth += 1;
            depths[index] = depth;
        } else if CLOSING.contains(&c) {
            depths[index] = depth;
            depth -= 1;
        } else {
            depths[index] = depth;
        }
        index += 1;
    }
    depths
}

// The index just past the string or comment that starts at `start`.
pub(crate) fn literal_end(text: &[char], start: usize) -> usize {
    if text[start] == COMMENT {
        return find(text, &['\n'], start).unwrap_or(text.len());
    }
    match literal_at(text, start) {
        Literal::Formatted(marker) => fstring_end(text, start, marker),
        Literal::Plain(marker) => plain_end(text, start, marker),
    }
}

// A string literal at a quote, by the length of its quote marker.
enum Literal {
    Plain(usize),
    Formatted(usize),
}

fn literal_at(text: &[char], start: usize) -> Literal {
    let triple = text
        .get(start..start + TRIPLE)
        .is_some_and(|run| QUOTES.contains(&run[0]) && run.iter().all(|&c| c == run[0]));
    let marker = if triple { TRIPLE } else { 1 };
    if opens_fstring(text, start) {
        Literal::Formatted(marker)
    } else {
        Literal::Plain(marker)
    }
}

fn plain_end(text: &[char], start: usize, marker: usize) -> usize {
    if marker == TRIPLE {
        let quote = &text[start..start + TRIPLE];
        return find(text, quote, start + TRIPLE).map_or(text.len(), |found| found + TRIPLE);
    }
    let quote = text[start];
    let mut index = start + 1;
    while index < text.len() {
        let c = text[index];
        if c == '\\' {
            index += 2;
            continue;
        }
        if c == quote || c == '\n' {
            return index + 1;
        }
        index += 1;
    }
    text.len()
}

// Whether the literal at `start` has an `f` among its prefix letters, and no word character
// before them.
fn opens_fstring(text: &[char], start: usize) -> bool {
    let prefix_start = text[..start]
        .iter()
        .rposition(|c| !STRING_PREFIXES.contains(c))
        .map_or(0, |at| at + 1);
    if prefix_start > 0 && text::is_word(text[prefix_start - 1]) {
        return false;
    }
    text[prefix_start..start]
        .iter()
        .any(|&c| matches!(c, 'f' | 'F'))
}

// One f-string being scanned: where its quote marker starts, the marker's length, the scan
// position, and the replacement-field depth.
#[derive(Clone, Copy)]
struct Frame {
    start: usize,
    marker: usize,
    index: usize,
    depth: usize,
}

impl Frame {
    fn opened(start: usize, marker: usize) -> Frame {
        Frame {
            start,
            marker,
            index: start + marker,
            depth: 0,
        }
    }

    // The step that moves the scan `by` characters at the same depth.
    fn skip(self, by: usize) -> Step {
        Step::Advance {
            index: self.index + by,
            depth: self.depth,
        }
    }
}

enum Step {
    Advance { index: usize, depth: usize },
    Nested(Frame),
    End(usize),
}

// A replacement field may hold a literal of its own, so the scan keeps a stack of f-strings
// instead of recursing: an untrusted block cannot exhaust the call stack.
fn fstring_end(text: &[char], start: usize, marker: usize) -> usize {
    let mut frames = vec![Frame::opened(start, marker)];
    while let Some(frame) = frames.last_mut() {
        match fstring_step(text, *frame) {
            Step::Advance { index, depth } => {
                frame.index = index;
                frame.depth = depth;
            }
            Step::Nested(nested) => frames.push(nested),
            Step::End(end) => {
                frames.pop();
                match frames.last_mut() {
                    Some(outer) => outer.index = end,
                    None => return end,
                }
            }
        }
    }
    text.len()
}

fn fstring_step(text: &[char], frame: Frame) -> Step {
    let Some(&c) = text.get(frame.index) else {
        return Step::End(text.len());
    };
    if c == '\\' {
        return frame.skip(2);
    }
    let marker = &text[frame.start..frame.start + frame.marker];
    if frame.depth == 0 && text[frame.index..].starts_with(marker) {
        return Step::End(frame.index + frame.marker);
    }
    match c {
        '{' | '}' => brace_step(text, frame, c),
        _ => field_step(text, frame, c),
    }
}

// A doubled brace is an escaped one only in the literal portion; inside a field `}}` closes a
// nested format spec and then the field.
fn brace_step(text: &[char], frame: Frame, c: char) -> Step {
    if frame.depth == 0 && text.get(frame.index + 1) == Some(&c) {
        return frame.skip(2);
    }
    let depth = if c == '{' {
        frame.depth + 1
    } else {
        frame.depth.saturating_sub(1)
    };
    Step::Advance {
        index: frame.index + 1,
        depth,
    }
}

// Inside a field a quote opens a literal of its own; outside one a line break ends a one-line
// f-string.
fn field_step(text: &[char], frame: Frame, c: char) -> Step {
    if frame.depth > 0 && QUOTES.contains(&c) {
        return match literal_at(text, frame.index) {
            Literal::Formatted(nested) => Step::Nested(Frame::opened(frame.index, nested)),
            Literal::Plain(nested) => Step::Advance {
                index: plain_end(text, frame.index, nested),
                depth: frame.depth,
            },
        };
    }
    if c == '\n' && frame.marker != TRIPLE {
        return Step::End(frame.index + 1);
    }
    frame.skip(1)
}

fn find(text: &[char], needle: &[char], from: usize) -> Option<usize> {
    text.get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| at + from)
}

// The text between the bracket at `offset` and the next closing bracket at its depth.
pub(crate) fn inner_span<'t>(
    text: &'t [char],
    depths: &[i64],
    offset: usize,
) -> Option<&'t [char]> {
    let wanted = *depths.get(offset)?;
    if wanted == INSIDE_STRING {
        return None;
    }
    let close = (offset + 1..text.len())
        .find(|&index| CLOSING.contains(&text[index]) && depths[index] == wanted)?;
    Some(&text[offset + 1..close])
}

// `text` split on the commas outside every bracket and string, each part stripped.
fn split_top_level(text: &[char]) -> Vec<&[char]> {
    let depths = structure(text);
    let mut parts = Vec::new();
    let mut start = 0;
    for (index, &c) in text.iter().enumerate() {
        if c == ',' && depths[index] == 0 {
            parts.push(trimmed(&text[start..index]));
            start = index + 1;
        }
    }
    parts.push(trimmed(&text[start..]));
    parts
}

fn trimmed(part: &[char]) -> &[char] {
    let start = part
        .iter()
        .position(|&c| !text::is_space(c))
        .unwrap_or(part.len());
    let end = part
        .iter()
        .rposition(|&c| !text::is_space(c))
        .map_or(start, |at| at + 1);
    &part[start..end]
}

// A `lambda` outside every bracket and string puts its parameter commas where the argument
// commas are, so a text that holds one is not judged.
fn holds_top_level_lambda(text: &[char]) -> bool {
    let depths = structure(text);
    let word = |at: Option<&char>| at.is_some_and(|&c| text::is_word(c));
    (0..text.len()).any(|at| {
        text[at..].starts_with(&LAMBDA)
            && depths[at] == 0
            && !word(at.checked_sub(1).and_then(|before| text.get(before)))
            && !word(text.get(at + LAMBDA.len()))
    })
}

// Whether `text` is a `name=value` argument: an `=` outside brackets that is not part of `==`,
// `!=`, `<=` or `>=`, and has a character before it.
fn names_keyword_argument(text: &[char]) -> bool {
    let depths = structure(text);
    text.iter().enumerate().any(|(index, &c)| {
        c == '='
            && depths[index] == 0
            && index > 0
            && !matches!(text[index - 1], '=' | '!' | '<' | '>')
            && text.get(index + 1) != Some(&'=')
    })
}

// An empty part or a part that starts with a star: the count on the page is not the count the
// callee sees.
fn unreadable(part: &[char]) -> bool {
    part.first().is_none_or(|&c| c == '*')
}

// One call's positional arguments, or `None` when the count on the page need not be the count
// the callee sees: no bracket span, a top-level lambda, a splat, a keyword or an empty argument.
pub(crate) fn positional_arguments(
    text: &[char],
    depths: &[i64],
    offset: usize,
) -> Option<Vec<String>> {
    let inner = inner_span(text, depths, offset)?;
    if holds_top_level_lambda(inner) {
        return None;
    }
    let arguments = split_top_level(inner);
    if let [only] = arguments.as_slice()
        && only.is_empty()
    {
        return Some(Vec::new());
    }
    let refused = |part: &&[char]| unreadable(part) || names_keyword_argument(part);
    if arguments.iter().any(refused) {
        return None;
    }
    Some(arguments.iter().map(|part| part.iter().collect()).collect())
}

// The text inside a signature's first balanced bracket pair; none for a truncated signature.
fn parameter_list(flattened: &[char]) -> Option<&[char]> {
    if flattened.ends_with(&['.', '.', '.']) {
        return None;
    }
    let opening = flattened.iter().position(|&c| c == '(')?;
    inner_span(flattened, &structure(flattened), opening)
}

// What a signature accepts positionally, or `None` for a signature with a star or a lambda
// default. The positional-only `/` marks how parameters are spelled, so it is dropped.
pub(crate) fn parameter_shape(signature: &str) -> Option<Shape> {
    let flattened: Vec<char> = text::strip(signature).chars().collect();
    let inner = parameter_list(&flattened)?;
    if trimmed(inner).is_empty() {
        return Some(Shape {
            required: 0,
            total: 0,
        });
    }
    if holds_top_level_lambda(inner) {
        return None;
    }
    let parameters: Vec<&[char]> = split_top_level(inner)
        .into_iter()
        .filter(|part| *part != POSITIONAL_ONLY)
        .collect();
    if parameters.iter().any(|part| unreadable(part)) {
        return None;
    }
    Some(Shape {
        required: parameters
            .iter()
            .filter(|part| !names_keyword_argument(part))
            .count(),
        total: parameters.len(),
    })
}

// The parameter names a captured signature declares.
pub(crate) fn signature_names(signature: &str) -> BTreeSet<String> {
    let chars: Vec<char> = signature.chars().collect();
    let Some(opening) = chars.iter().position(|&c| c == '(') else {
        return BTreeSet::new();
    };
    let Some(inner) = inner_span(&chars, &structure(&chars), opening) else {
        return BTreeSet::new();
    };
    split_top_level(inner)
        .into_iter()
        .filter_map(|part| {
            let part: String = part.iter().collect();
            let head = part.split('=').next().unwrap_or_default();
            let head = text::strip(head.split(':').next().unwrap_or_default());
            let head = text::strip(head.trim_start_matches('*'));
            text::is_identifier(head).then(|| head.to_owned())
        })
        .collect()
}
