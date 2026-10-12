//! The parse of one command line: each argument classified as an option or a value, then the
//! options and the subcommand consumed from left to right.

use std::collections::BTreeMap;

use text::CodePointString;

use super::help::{help_text, usage_text};
use super::{Arg, Chosen, Command, Exit, Kind, Namespace, Parsed, Parser, Value};

// One reading of an option argument: the action it names (`None` for an unknown option), the
// option string, whether `=` gave the value, and the value written in the same argument.
struct Tuple {
    action: Option<usize>,
    option: String,
    equals: bool,
    explicit: Option<Arg>,
}

struct Known {
    namespace: Namespace,
    extras: Vec<String>,
    chosen: Option<Chosen>,
}

pub(crate) fn parse(top: &Parser, args: &[Arg], width: isize) -> Result<Parsed, Exit> {
    let known = parse_known(top, args, width)?;
    let chosen = known.chosen.expect("the top parser requires a command");
    if !matches!(chosen, Chosen::Unported(_)) && !known.extras.is_empty() {
        let message = format!("unrecognized arguments: {}", known.extras.join(" "));
        return Err(error(top, width, &message));
    }
    Ok(Parsed {
        top: known.namespace,
        chosen,
    })
}

fn parse_known(parser: &Parser, args: &[Arg], width: isize) -> Result<Known, Exit> {
    let (pattern, options) = classify(parser, args);
    let positional = parser.actions.iter().position(|a| a.strings.is_empty());
    let mut run = Run {
        parser,
        args,
        width,
        pattern,
        options,
        positional,
        namespace: Namespace::default(),
        seen: vec![false; parser.actions.len()],
        used: vec![false; parser.actions.len()],
        extras: Vec::new(),
        sub_extras: Vec::new(),
        chosen: None,
    };
    run.consume_all()?;
    run.check_required()?;
    run.extras.append(&mut run.sub_extras);
    Ok(Known {
        namespace: run.namespace,
        extras: run.extras,
        chosen: run.chosen,
    })
}

// The pattern holds `O` for an option, `A` for a value, and `-` for the `--` that makes every
// later argument a value.
fn classify(parser: &Parser, args: &[Arg]) -> (Vec<u8>, BTreeMap<usize, Vec<Tuple>>) {
    let mut pattern = Vec::with_capacity(args.len());
    let mut options = BTreeMap::new();
    for (index, arg) in args.iter().enumerate() {
        if pattern.contains(&b'-') {
            pattern.push(b'A');
        } else if arg.text == "--" {
            pattern.push(b'-');
        } else if let Some(tuples) = optional(parser, arg) {
            options.insert(index, tuples);
            pattern.push(b'O');
        } else {
            pattern.push(b'A');
        }
    }
    (pattern, options)
}

// The readings of `arg` as an option, or `None` when it is a value.
fn optional(parser: &Parser, arg: &Arg) -> Option<Vec<Tuple>> {
    let text = arg.text.as_str();
    if !text.starts_with('-') {
        return None;
    }
    if let Some(action) = parser.option(text) {
        return Some(vec![tuple(Some(action), text, false, None)]);
    }
    if text.chars().count() == 1 {
        return None;
    }
    if let Some((name, _)) = text.split_once('=')
        && let Some(action) = parser.option(name)
    {
        return Some(vec![tuple(
            Some(action),
            name,
            true,
            Some(arg.after_equals()),
        )]);
    }
    let tuples = prefixed(parser, arg);
    if !tuples.is_empty() {
        return Some(tuples);
    }
    if negative_number(text) || text.contains(' ') {
        return None;
    }
    Some(vec![tuple(None, text, false, None)])
}

fn tuple(action: Option<usize>, option: &str, equals: bool, explicit: Option<Arg>) -> Tuple {
    Tuple {
        action,
        option: option.to_owned(),
        equals,
        explicit,
    }
}

// The option strings that `arg` abbreviates. A single-dash argument also reads as its first two
// characters followed by a value.
fn prefixed(parser: &Parser, arg: &Arg) -> Vec<Tuple> {
    let text = arg.text.as_str();
    let (prefix, explicit) = match text.split_once('=') {
        Some((prefix, _)) => (prefix, Some(arg.after_equals())),
        None => (text, None),
    };
    let equals = explicit.is_some();
    if text.starts_with("--") {
        let matching = parser
            .option_strings()
            .filter(|(_, s)| s.starts_with(prefix));
        return matching
            .map(|(i, s)| tuple(Some(i), s, equals, explicit.clone()))
            .collect();
    }
    let split = text.char_indices().nth(2).map_or(text.len(), |(at, _)| at);
    let (short, rest) = text.split_at(split);
    let rest = Arg::new(rest.as_ref());
    let readings = parser.option_strings().filter_map(|(i, s)| {
        if s == short {
            Some(tuple(Some(i), s, false, Some(rest.clone())))
        } else {
            s.starts_with(prefix)
                .then(|| tuple(Some(i), s, equals, explicit.clone()))
        }
    });
    readings.collect()
}

// `-` and then digits, with an optional decimal point; one final newline is allowed.
fn negative_number(text: &str) -> bool {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let Some(number) = text.strip_prefix('-') else {
        return false;
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    if number.contains('.') {
        digits(whole) && !fraction.is_empty() && digits(fraction)
    } else {
        !whole.is_empty() && digits(whole)
    }
}

struct Run<'p> {
    parser: &'p Parser,
    args: &'p [Arg],
    width: isize,
    pattern: Vec<u8>,
    options: BTreeMap<usize, Vec<Tuple>>,
    positional: Option<usize>,
    namespace: Namespace,
    seen: Vec<bool>,
    used: Vec<bool>,
    extras: Vec<String>,
    sub_extras: Vec<String>,
    chosen: Option<Chosen>,
}

impl Run<'_> {
    // Consumes values and options alternately until no option is left, then the last values.
    fn consume_all(&mut self) -> Result<(), Exit> {
        let mut start = 0;
        while let Some(next) = self.options.range(start..).next().map(|(&at, _)| at) {
            start = match self.consume_values(start, next)? {
                Some(end) => end,
                None => self.consume_optional(next)?,
            };
        }
        let stop = self.consume_positional(start)?;
        self.extras.extend(self.texts(stop..self.args.len()));
        Ok(())
    }

    // The values before the option at `next`: the subcommand takes them with every later
    // argument and gives the end, or they become extras.
    fn consume_values(&mut self, start: usize, next: usize) -> Result<Option<usize>, Exit> {
        if start == next {
            return Ok(None);
        }
        let end = self.consume_positional(start)?;
        if end > start {
            return Ok(Some(end));
        }
        self.extras.extend(self.texts(start..next));
        Ok(None)
    }

    fn texts(&self, range: std::ops::Range<usize>) -> Vec<String> {
        self.args[range]
            .iter()
            .map(|arg| arg.text.clone())
            .collect()
    }

    // The subcommand takes its name and every argument after it; a leading `--` is dropped.
    fn consume_positional(&mut self, start: usize) -> Result<usize, Exit> {
        let skip = usize::from(self.pattern.get(start) == Some(&b'-'));
        if self.pattern.get(start + skip) != Some(&b'A') {
            return Ok(start);
        }
        let Some(index) = self.positional.take() else {
            return Ok(start);
        };
        let args = self.args;
        self.take_action(index, &args[start + skip..])?;
        Ok(args.len())
    }

    fn consume_optional(&mut self, start: usize) -> Result<usize, Exit> {
        let tuples = self.options.remove(&start).unwrap_or_default();
        if tuples.len() > 1 {
            let names: Vec<&str> = tuples.iter().map(|t| t.option.as_str()).collect();
            let shown = &self.args[start].text;
            let message = format!("ambiguous option: {shown} could match {}", names.join(", "));
            return Err(self.error(&message));
        }
        let Some(first) = tuples.into_iter().next() else {
            return Ok(start + 1);
        };
        let (taken, stop) = self.match_optional(start, first)?;
        for (index, values) in taken {
            self.take_action(index, &values)?;
        }
        Ok(stop)
    }

    // The actions that the option at `start` names, with their values, and the index after them.
    fn match_optional(&mut self, start: usize, first: Tuple) -> Result<Taken, Exit> {
        let mut taken = Vec::new();
        let mut reading = first;
        loop {
            let Some(index) = reading.action else {
                self.extras.push(self.args[start].text.clone());
                return Ok((taken, start + 1));
            };
            let Some(value) = reading.explicit.take() else {
                let count = self.match_argument(index, start + 1)?;
                taken.push((index, self.args[start + 1..start + 1 + count].to_vec()));
                return Ok((taken, start + 1 + count));
            };
            if self.takes_value(index) {
                taken.push((index, vec![value]));
                return Ok((taken, start + 1));
            }
            taken.push((index, Vec::new()));
            let Some(next) = self.next_short(index, &reading, &value)? else {
                self.extras.push(format!("-{}", value.text));
                return Ok((taken, start + 1));
            };
            reading = next;
        }
    }

    // After a short flag that takes no value, the next character of the same argument names the
    // next short option.
    fn next_short(
        &self,
        index: usize,
        reading: &Tuple,
        value: &Arg,
    ) -> Result<Option<Tuple>, Exit> {
        let long = reading.option.as_bytes().get(1) == Some(&b'-');
        if long || value.text.is_empty() || reading.equals || value.text.starts_with('-') {
            let shown = CodePointString::from(value.text.as_str()).quoted();
            return Err(self.action_error(index, &format!("ignored explicit argument {shown}")));
        }
        let mut chars = value.text.chars();
        let option = format!("-{}", chars.next().unwrap_or_default());
        let rest = chars.as_str();
        let Some(next) = self.parser.option(&option) else {
            return Ok(None);
        };
        let (equals, explicit) = match rest.strip_prefix('=') {
            _ if rest.is_empty() => (false, None),
            Some(after) => (true, Some(Arg::new(after.as_ref()))),
            None => (false, Some(Arg::new(rest.as_ref()))),
        };
        Ok(Some(tuple(Some(next), &option, equals, explicit)))
    }

    fn takes_value(&self, index: usize) -> bool {
        matches!(self.parser.actions[index].kind, Kind::Value { .. })
    }

    // The count of values the action at `index` takes from `from` on: one for a value option.
    fn match_argument(&self, index: usize, from: usize) -> Result<usize, Exit> {
        if !self.takes_value(index) {
            return Ok(0);
        }
        if self.pattern.get(from) == Some(&b'A') {
            Ok(1)
        } else {
            Err(self.action_error(index, "expected one argument"))
        }
    }

    fn take_action(&mut self, index: usize, values: &[Arg]) -> Result<(), Exit> {
        self.seen[index] = true;
        let value = self.convert(index, values)?;
        let action = &self.parser.actions[index];
        if !action.strings.is_empty() || !values.is_empty() {
            self.used[index] = true;
            if let Some(other) = self.parser.conflicts(index).find(|&o| self.used[o]) {
                let other = self.parser.actions[other].name();
                return Err(self.action_error(index, &format!("not allowed with argument {other}")));
            }
        }
        match action.kind {
            Kind::Help => Err(Exit::Help(help_text(self.parser, self.width))),
            Kind::Flag | Kind::Value { .. } => {
                self.namespace.set(action.dest, value);
                Ok(())
            }
            Kind::Commands(commands) => self.choose(commands, values),
        }
    }

    // The value of the action at `index`: its conversion, then its choices.
    fn convert(&self, index: usize, values: &[Arg]) -> Result<Value, Exit> {
        let (value, choices) = match &self.parser.actions[index].kind {
            Kind::Help | Kind::Flag => return Ok(Value::Flag),
            Kind::Value { convert, choices } => {
                let converted = convert(&values[0]);
                (
                    converted.map_err(|m| self.action_error(index, &m))?,
                    choices.to_vec(),
                )
            }
            Kind::Commands(commands) => {
                let names = commands.iter().map(|command| command.name).collect();
                (Value::Text(values[0].clone()), names)
            }
        };
        let text = &values[0].text;
        if choices.is_empty() || choices.contains(&text.as_str()) {
            return Ok(value);
        }
        let shown = CodePointString::from(text.as_str()).quoted();
        let message = format!(
            "invalid choice: {shown} (choose from {})",
            choices.join(", ")
        );
        Err(self.action_error(index, &message))
    }

    fn choose(&mut self, commands: &'static [Command], values: &[Arg]) -> Result<(), Exit> {
        let Some(command) = commands.iter().find(|c| c.name == values[0].text) else {
            return Ok(());
        };
        self.chosen = Some(match command.parser {
            None => Chosen::Unported(command.name),
            Some(build) => {
                let known = parse_known(&build(), &values[1..], self.width)?;
                self.sub_extras = known.extras;
                Chosen::Ported {
                    name: command.name,
                    namespace: known.namespace,
                }
            }
        });
        Ok(())
    }

    fn check_required(&self) -> Result<(), Exit> {
        let missing: Vec<String> = (self.parser.actions.iter().enumerate())
            .filter(|(index, action)| {
                matches!(action.kind, Kind::Commands(_)) && !self.seen[*index]
            })
            .map(|(_, action)| action.name())
            .collect();
        if !missing.is_empty() {
            let message = format!(
                "the following arguments are required: {}",
                missing.join(", ")
            );
            return Err(self.error(&message));
        }
        let Some(group) = self.parser.group.as_ref().filter(|group| group.required) else {
            return Ok(());
        };
        if group.members.clone().any(|member| self.used[member]) {
            return Ok(());
        }
        let names: Vec<String> = group
            .members
            .clone()
            .map(|m| self.parser.actions[m].name())
            .collect();
        Err(self.error(&format!(
            "one of the arguments {} is required",
            names.join(" ")
        )))
    }

    fn error(&self, message: &str) -> Exit {
        error(self.parser, self.width, message)
    }

    fn action_error(&self, index: usize, message: &str) -> Exit {
        let name = self.parser.actions[index].name();
        self.error(&format!("argument {name}: {message}"))
    }
}

type Taken = (Vec<(usize, Vec<Arg>)>, usize);

fn error(parser: &Parser, width: isize, message: &str) -> Exit {
    let usage = usage_text(parser, width);
    Exit::Usage(format!("{usage}{}: error: {message}\n", parser.prog))
}
