use syntax::Language;
use text::CodePointString;

use crate::slices::merge_intervals;
use crate::stable_id::{StableId, split_ordinal};
use crate::values::{Symbol, SymbolKind};

/// The lines that a location's span is widened by on each side, by default.
pub const DEFAULT_CONTEXT_LINES: u32 = 10;

const CLASS_KINDS: [SymbolKind; 4] = [
    SymbolKind::Class,
    SymbolKind::Dataclass,
    SymbolKind::Enum,
    SymbolKind::Protocol,
];
const UNKNOWN_FORM: &str = "unknown location form: expected class:, function:, line: or variable:";
// int() refuses a decimal text of more digits than this.
const MAX_INT_DIGITS: usize = 4300;

/// The one file that a set of locations resolves against.
#[derive(Clone, Copy, Debug)]
pub struct LocTarget<'a> {
    pub path: &'a str,
    pub total_lines: u32,
    /// The file's symbols; none for a file that no grammar parses.
    pub file: Option<FileSymbols<'a>>,
}

/// The symbols of one file and the language they were extracted in.
#[derive(Clone, Copy, Debug)]
pub struct FileSymbols<'a> {
    pub language: Language,
    pub symbols: &'a [Symbol],
}

/// What a set of location strings resolved to: the ids of the matched symbols, one span per
/// resolved location, the widened and merged intervals, and each location that did not resolve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocResolution {
    pub stable_ids: Vec<StableId>,
    pub spans: Vec<(u32, u32)>,
    pub intervals: Vec<(u32, u32)>,
    pub unrecognized: Vec<UnrecognizedLoc>,
}

/// A location string that resolved to nothing, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnrecognizedLoc {
    pub loc: String,
    pub reason: String,
}

/// Resolve `class:`, `function:`, `method:`, `line:` and `variable:` locations, one per line of
/// each entry, against one file; a `class:` location sets the class of the bare names after it.
#[must_use]
pub fn resolve_locs(locs: &[String], target: &LocTarget<'_>, context: u32) -> LocResolution {
    let resolver = Resolver { target };
    let mut resolution = LocResolution {
        stable_ids: Vec::new(),
        spans: Vec::new(),
        intervals: Vec::new(),
        unrecognized: Vec::new(),
    };
    let mut current_class = String::new();
    for loc in split(locs) {
        match resolver.resolve(loc, &current_class) {
            Hit::Refused(reason) => resolution.unrecognized.push(UnrecognizedLoc {
                loc: loc.to_owned(),
                reason,
            }),
            Hit::Found { span, id, class } => {
                current_class = class.unwrap_or(current_class);
                resolution.spans.push(span);
                resolution.stable_ids.extend(id);
            }
        }
    }
    resolution.stable_ids = deduplicated(resolution.stable_ids);
    resolution.intervals = merge_intervals(&widened(&resolution.spans, target, context));
    resolution
}

// One location per non-blank line of each entry, without its surrounding whitespace.
fn split(locs: &[String]) -> Vec<&str> {
    locs.iter()
        .flat_map(|block| text::splitlines(block, false))
        .map(text::strip)
        .filter(|line| !line.is_empty())
        .collect()
}

fn deduplicated(ids: Vec<StableId>) -> Vec<StableId> {
    let mut kept: Vec<StableId> = Vec::new();
    for id in ids {
        if !kept.contains(&id) {
            kept.push(id);
        }
    }
    kept
}

// Each span widened by `context` and clipped to the file; a span wholly past the end gives none.
fn widened(spans: &[(u32, u32)], target: &LocTarget<'_>, context: u32) -> Vec<(u32, u32)> {
    spans
        .iter()
        .map(|(start, end)| {
            let low = start.saturating_sub(context).max(1);
            (low, end.saturating_add(context).min(target.total_lines))
        })
        .filter(|(low, high)| low <= high)
        .collect()
}

enum Hit {
    Found {
        span: (u32, u32),
        id: Option<StableId>,
        class: Option<String>,
    },
    Refused(String),
}

struct Resolver<'t, 'a> {
    target: &'t LocTarget<'a>,
}

impl Resolver<'_, '_> {
    // `line:` and `variable:` come before the dotted-name rule, so `line: 7 in A.b` is a line.
    fn resolve(&self, loc: &str, current_class: &str) -> Hit {
        let rest = text::strip(loc.split_once(':').map_or("", |(_, rest)| rest));
        if loc.starts_with("class:") && !loc.contains('.') {
            return self.class(rest);
        }
        if loc.starts_with("line:") {
            return self.line(rest);
        }
        if loc.starts_with("variable:") {
            return self.variable(rest);
        }
        if loc.starts_with("function:") || loc.starts_with("method:") || loc.contains('.') {
            let name = if rest.is_empty() { loc } else { rest };
            return self.function(name, current_class);
        }
        Hit::Refused(UNKNOWN_FORM.to_owned())
    }

    fn symbols(&self) -> &[Symbol] {
        self.target.file.map_or(&[], |file| file.symbols)
    }

    fn found(&self, symbol: &Symbol) -> Hit {
        self.found_in(symbol, None)
    }

    // The symbol's span and id, and the class that the locations after it fall back on.
    fn found_in(&self, symbol: &Symbol, class: Option<String>) -> Hit {
        let id = self
            .target
            .file
            .map(|file| StableId::of(file.language, self.target.path, symbol));
        Hit::Found {
            span: (symbol.line, symbol.end_line),
            id,
            class,
        }
    }

    fn class(&self, name: &str) -> Hit {
        let first = self
            .symbols()
            .iter()
            .find(|symbol| CLASS_KINDS.contains(&symbol.kind) && symbol.name == name);
        match first {
            Some(symbol) => self.found_in(symbol, Some(name.to_owned())),
            None => Hit::Refused(format!(
                "no class named {} in {}",
                repr(name),
                self.target.path
            )),
        }
    }

    // A trailing `#N` addresses the Nth symbol of a name that the file spells more than once.
    fn function(&self, name: &str, current_class: &str) -> Hit {
        let (wanted, ordinal) = split_ordinal(name);
        match wanted.rsplit_once('.') {
            Some((owner, member)) => self.dotted(wanted, owner, member, ordinal),
            None => self.bare(wanted, ordinal, current_class),
        }
    }

    // `outer.inner` names a nested function before `Class.method` names a method.
    fn dotted(&self, wanted: &str, owner: &str, member: &str, ordinal: u32) -> Hit {
        let nested: Vec<&Symbol> = self
            .symbols()
            .iter()
            .filter(|symbol| {
                symbol.kind == SymbolKind::Function
                    && symbol.parent == owner
                    && symbol.name == member
            })
            .collect();
        if nested.is_empty() {
            return self.method(owner, member, ordinal);
        }
        self.pick(&nested, wanted, ordinal)
    }

    // A module function, else a method of the current class, else the one method of that name.
    fn bare(&self, wanted: &str, ordinal: u32, current_class: &str) -> Hit {
        let functions: Vec<&Symbol> = self
            .symbols()
            .iter()
            .filter(|symbol| symbol.kind == SymbolKind::Function && symbol.name == wanted)
            .collect();
        if !functions.is_empty() {
            return self.pick(&functions, wanted, ordinal);
        }
        if !current_class.is_empty() {
            let hit = self.method(current_class, wanted, ordinal);
            if matches!(hit, Hit::Found { .. }) {
                return hit;
            }
        }
        self.unqualified_method(wanted, ordinal)
    }

    fn method(&self, class: &str, member: &str, ordinal: u32) -> Hit {
        let symbols = self.symbols();
        let defined = symbols
            .iter()
            .any(|symbol| CLASS_KINDS.contains(&symbol.kind) && symbol.name == class);
        if !defined {
            return Hit::Refused(format!(
                "no class named {} in {}",
                repr(class),
                self.target.path
            ));
        }
        let methods: Vec<&Symbol> = symbols
            .iter()
            .filter(|symbol| symbol.parent == class && symbol.name == member)
            .collect();
        if methods.is_empty() {
            return Hit::Refused(format!(
                "class {} has no member named {}",
                repr(class),
                repr(member)
            ));
        }
        self.pick(&methods, &format!("{class}.{member}"), ordinal)
    }

    // A bare method name resolves only when exactly one class defines it.
    fn unqualified_method(&self, name: &str, ordinal: u32) -> Hit {
        let methods: Vec<&Symbol> = self
            .symbols()
            .iter()
            .filter(|symbol| !symbol.parent.is_empty() && symbol.name == name)
            .collect();
        if methods.is_empty() {
            let path = self.target.path;
            return Hit::Refused(format!(
                "no function or method named {} in {path}",
                repr(name)
            ));
        }
        if methods.len() > 1 {
            let mut owners: Vec<&str> = methods
                .iter()
                .map(|symbol| symbol.parent.as_str())
                .collect();
            owners.sort_unstable();
            owners.dedup();
            let owners = owners.join(", ");
            return Hit::Refused(format!("{} is ambiguous: defined in {owners}", repr(name)));
        }
        self.pick(&methods, name, ordinal)
    }

    fn pick(&self, matches: &[&Symbol], name: &str, ordinal: u32) -> Hit {
        match matches
            .iter()
            .find(|symbol| symbol.duplicate_index == ordinal)
        {
            Some(chosen) => self.found(chosen),
            None => Hit::Refused(format!(
                "{} defines {} symbols named {}, so there is no number {}",
                self.target.path,
                matches.len(),
                repr(name),
                u64::from(ordinal) + 1
            )),
        }
    }

    // `line: N` takes the first word as the number and ignores the rest.
    fn line(&self, text: &str) -> Hit {
        let Some(head) = text::split_whitespace(text).next() else {
            return Hit::Refused("line location carries no number".to_owned());
        };
        let Some(number) = LineNumber::read(head) else {
            return Hit::Refused(format!("{} is not a line number", repr(head)));
        };
        match number.within(self.target.total_lines) {
            Some(line) => Hit::Found {
                span: (line, line),
                id: None,
                class: None,
            },
            None => Hit::Refused(format!(
                "line {} is outside {} (1-{})",
                number.text, self.target.path, self.target.total_lines
            )),
        }
    }

    fn variable(&self, text: &str) -> Hit {
        let Some(name) = text::split_whitespace(text).next() else {
            return Hit::Refused("variable location carries no name".to_owned());
        };
        let constant = self
            .symbols()
            .iter()
            .find(|symbol| symbol.kind == SymbolKind::Constant && symbol.name == name);
        match constant {
            Some(symbol) => self.found(symbol),
            None => Hit::Refused(format!(
                "no module-level constant named {} in {}",
                repr(name),
                self.target.path
            )),
        }
    }
}

// A decimal integer as int() reads it: its value printed in full, and whether it is above zero.
struct LineNumber {
    text: String,
    positive: bool,
}

impl LineNumber {
    fn read(head: &str) -> Option<LineNumber> {
        let found = text::int_text(head)?;
        let digits: String = found
            .digits()
            .map(|digit| char::from(b'0' + digit))
            .collect();
        if digits.len() > MAX_INT_DIGITS {
            return None;
        }
        let digits = digits.trim_start_matches('0');
        let text = match (digits.is_empty(), found.negative) {
            (true, _) => "0".to_owned(),
            (false, true) => format!("-{digits}"),
            (false, false) => digits.to_owned(),
        };
        Some(LineNumber {
            positive: !digits.is_empty() && !found.negative,
            text,
        })
    }

    // The line, when the number is one of the file's lines.
    fn within(&self, total: u32) -> Option<u32> {
        let line: u32 = self.text.parse().ok()?;
        (self.positive && line <= total).then_some(line)
    }
}

fn repr(text: &str) -> String {
    CodePointString::from(text).quoted()
}
