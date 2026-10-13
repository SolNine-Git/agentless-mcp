//! The usage line and the help text of a parser, wrapped to the terminal width.

use std::ops::Range;

use app::ops::Arity;

use super::wrap::{collapse, wrap};
use super::{Action, Kind, Parser};

const PREFIX: &str = "usage: ";

pub(crate) fn usage_text(parser: &Parser, width: isize) -> String {
    format!("{PREFIX}{}\n", usage_body(parser, width))
}

pub(crate) fn help_text(parser: &Parser, width: isize) -> String {
    let (optionals, positionals) = split(parser);
    let layout = Layout::new(parser, width);
    let mut text = format!("{PREFIX}{}\n\n", usage_body(parser, width));
    if let Some(description) = parser.description {
        let description = collapse(description);
        let lines = wrap(&description, to_width(width.max(11)));
        text.push_str(&lines.join("\n"));
        text.push_str("\n\n");
    }
    for (title, indices) in [
        ("positional arguments", positionals),
        ("options", optionals),
    ] {
        if indices.is_empty() {
            continue;
        }
        text.push('\n');
        text.push_str(title);
        text.push_str(":\n");
        for index in indices {
            text.push_str(&layout.action(&parser.actions[index], 2));
        }
        text.push('\n');
    }
    tidy(&text)
}

// Runs of three or more line breaks become one blank line, and the text ends with one break.
fn tidy(text: &str) -> String {
    let mut tidied = String::with_capacity(text.len());
    let mut breaks = 0;
    for c in text.trim_matches('\n').chars() {
        breaks = if c == '\n' { breaks + 1 } else { 0 };
        if breaks <= 2 {
            tidied.push(c);
        }
    }
    tidied.push('\n');
    tidied
}

fn usage_body(parser: &Parser, width: isize) -> String {
    let (optionals, positionals) = split(parser);
    let opt_parts = parts(parser, &optionals);
    let pos_parts = parts(parser, &positionals);
    let words: Vec<&str> = std::iter::once(parser.prog.as_str())
        .chain(opt_parts.iter().chain(&pos_parts).map(String::as_str))
        .collect();
    let usage = words.join(" ");
    if cols(PREFIX) + cols(&usage) <= width {
        return usage;
    }
    wrapped(&parser.prog, &opt_parts, &pos_parts, width).join("\n")
}

// A short program name keeps the parts beside it; a long one stands on its own line.
fn wrapped(prog: &str, opt_parts: &[String], pos_parts: &[String], width: isize) -> Vec<String> {
    let opt: Vec<&str> = opt_parts.iter().map(String::as_str).collect();
    let pos: Vec<&str> = pos_parts.iter().map(String::as_str).collect();
    let lead = PREFIX.len() + prog.len();
    if 4 * cols_of(lead) <= 3 * width {
        let indent = " ".repeat(lead + 1);
        let fill = Fill {
            indent: &indent,
            width,
        };
        if !opt.is_empty() {
            let mut lines = fill.lines(&[&[prog], opt.as_slice()].concat(), true);
            lines.extend(fill.lines(&pos, false));
            return lines;
        }
        if pos.is_empty() {
            return vec![prog.to_owned()];
        }
        return fill.lines(&[&[prog], pos.as_slice()].concat(), true);
    }
    let indent = " ".repeat(PREFIX.len());
    let fill = Fill {
        indent: &indent,
        width,
    };
    let mut lines = fill.lines(&[opt.as_slice(), &pos].concat(), false);
    if lines.len() > 1 {
        lines = fill.lines(&opt, false);
        lines.extend(fill.lines(&pos, false));
    }
    std::iter::once(prog.to_owned()).chain(lines).collect()
}

struct Fill<'a> {
    indent: &'a str,
    width: isize,
}

impl Fill<'_> {
    // Puts the parts on indented lines; the first line of the prefixed form follows `usage: `.
    fn lines(&self, parts: &[&str], prefixed: bool) -> Vec<String> {
        let mut lines = Vec::new();
        let mut line: Vec<&str> = Vec::new();
        let start = if prefixed {
            cols(PREFIX)
        } else {
            cols(self.indent)
        } - 1;
        let mut length = start;
        for part in parts {
            if length + 1 + cols(part) > self.width && !line.is_empty() {
                lines.push(format!("{}{}", self.indent, line.join(" ")));
                line.clear();
                length = cols(self.indent) - 1;
            }
            line.push(part);
            length += cols(part) + 1;
        }
        if !line.is_empty() {
            lines.push(format!("{}{}", self.indent, line.join(" ")));
        }
        if prefixed && let Some(first) = lines.first_mut() {
            first.drain(..self.indent.len());
        }
        lines
    }
}

fn split(parser: &Parser) -> (Vec<usize>, Vec<usize>) {
    (0..parser.actions.len()).partition(|&index| !parser.actions[index].strings.is_empty())
}

// The usage parts of the actions at `indices`: an optional in brackets, and the members of the
// mutually exclusive group joined with ` |` inside its parentheses.
fn parts(parser: &Parser, indices: &[usize]) -> Vec<String> {
    let span = group_span(parser, indices);
    let parts = indices.iter().enumerate().map(|(at, &index)| {
        let action = &parser.actions[index];
        let part = usage_part(action);
        let grouped = span.as_ref().is_some_and(|span| span.contains(&at));
        if action.strings.is_empty() || action.required || grouped {
            part
        } else {
            format!("[{part}]")
        }
    });
    let mut parts: Vec<String> = parts.collect();
    if let (Some(span), Some(group)) = (span, parser.group.as_ref()) {
        mark_group(&mut parts, span, group.required);
    }
    parts
}

// Where the group's members stand, in order and together, among `indices`.
fn group_span(parser: &Parser, indices: &[usize]) -> Option<Range<usize>> {
    let members = parser.group.as_ref()?.members.clone();
    let start = indices.iter().position(|&index| index == members.start)?;
    let end = start + members.len();
    let together = indices.get(start..end)?.iter().copied().eq(members);
    together.then_some(start..end)
}

fn mark_group(parts: &mut [String], span: Range<usize>, required: bool) {
    let (open, close) = match (required, span.len() > 1) {
        (true, true) => ("(", ")"),
        (true, false) => ("", ""),
        (false, _) => ("[", "]"),
    };
    parts[span.start].insert_str(0, open);
    parts[span.end - 1].push_str(close);
    for part in &mut parts[span.start..span.end - 1] {
        part.push_str(" |");
    }
}

fn usage_part(action: &Action) -> String {
    match action.kind {
        Kind::Help | Kind::Flag => action.strings[0].to_owned(),
        Kind::Value { .. } => format!("{} {}", action.strings[0], metavar(action)),
        Kind::Positional(arity) => positional_part(arity, &metavar(action)),
        Kind::Commands(_) => format!("{} ...", metavar(action)),
    }
}

// A positional's usage shows how many values it takes.
fn positional_part(arity: Arity, metavar: &str) -> String {
    match arity {
        Arity::One => metavar.to_owned(),
        Arity::Optional => format!("[{metavar}]"),
        Arity::Any => format!("[{metavar} ...]"),
        Arity::OneOrMore => format!("{metavar} [{metavar} ...]"),
    }
}

fn invocation(action: &Action) -> String {
    match action.kind {
        Kind::Help | Kind::Flag => action.strings.join(", "),
        Kind::Value { .. } => format!("{} {}", action.strings.join(", "), metavar(action)),
        Kind::Positional(_) | Kind::Commands(_) => metavar(action),
    }
}

fn metavar(action: &Action) -> String {
    if let Some(metavar) = action.metavar {
        return metavar.to_owned();
    }
    let choices: Vec<&str> = match action.kind {
        Kind::Value { choices, .. } => choices.to_vec(),
        Kind::Commands(commands) => commands.iter().map(|command| command.name).collect(),
        Kind::Help | Kind::Flag | Kind::Positional(_) => Vec::new(),
    };
    if choices.is_empty() {
        action.dest.to_uppercase()
    } else {
        format!("{{{}}}", choices.join(","))
    }
}

// The help column and the text width that every entry of one help text shares.
struct Layout {
    help_position: isize,
    width: isize,
}

impl Layout {
    fn new(parser: &Parser, width: isize) -> Layout {
        let entries = parser.actions.iter().flat_map(|action| {
            let own = cols(&invocation(action)) + 2;
            let commands = match action.kind {
                Kind::Commands(commands) => commands,
                _ => &[],
            };
            std::iter::once(own).chain(commands.iter().map(|command| cols(command.name) + 4))
        });
        let longest = entries.max().unwrap_or(0);
        let column_cap = (width - 20).clamp(4, 24);
        Layout {
            help_position: (longest + 2).min(column_cap),
            width,
        }
    }

    fn action(&self, action: &Action, indent: isize) -> String {
        let mut text = self.entry(&invocation(action), action.help.as_deref(), indent);
        if let Kind::Commands(commands) = action.kind {
            for command in commands {
                text.push_str(&self.entry(command.name, Some(command.help), indent + 2));
            }
        }
        text
    }

    // One entry: the invocation, padded to the help column when it fits, then the help lines.
    fn entry(&self, header: &str, help: Option<&str>, indent: isize) -> String {
        let pad = spaces(indent);
        let Some(help) = help.filter(|help| !help.is_empty()) else {
            return format!("{pad}{header}\n");
        };
        let action_width = self.help_position - indent - 2;
        let (mut text, first_indent) = if cols(header) <= action_width {
            let column = to_width(action_width);
            (format!("{pad}{header:<column$}  "), 0)
        } else {
            (format!("{pad}{header}\n"), self.help_position)
        };
        let help = collapse(help);
        let lines = wrap(&help, to_width((self.width - self.help_position).max(11)));
        if lines.is_empty() {
            text.push('\n');
        }
        for (at, line) in lines.into_iter().enumerate() {
            let indent = if at == 0 {
                first_indent
            } else {
                self.help_position
            };
            text.push_str(&spaces(indent));
            text.push_str(line);
            text.push('\n');
        }
        text
    }
}

fn spaces(count: isize) -> String {
    " ".repeat(to_width(count))
}

fn to_width(count: isize) -> usize {
    usize::try_from(count).unwrap_or(0)
}

fn cols(text: &str) -> isize {
    cols_of(text.len())
}

fn cols_of(length: usize) -> isize {
    isize::try_from(length).unwrap_or(isize::MAX)
}
