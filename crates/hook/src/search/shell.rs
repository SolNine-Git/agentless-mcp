//! The read-before-edit views in a shell command: the one file that a view of one file reads,
//! alone or after one `cd` to a literal folder. A shape the rules cannot read with certainty
//! reads no file.

use text::{CodePointString, posix_name, split_shell_code_points};

use crate::text::lossy;

const GREP_NAMES: [&str; 3] = ["grep", "egrep", "fgrep"];
const RG_NAMES: [&str; 2] = ["rg", "ripgrep"];
const SED_QUIET: &str = "-n";
// The characters of a pipe, a redirect, a command list and a command substitution.
const OPERATORS: [char; 6] = ['|', '&', ';', '<', '>', '`'];
const LINE_BREAKS: [char; 2] = ['\n', '\r'];
const END_OF_OPTIONS: &str = "--";
const CD: &str = "cd";
const AND: &str = "&&";
// The place of the `&&` among the words of `cd DIR && VIEW`.
const AND_AT: usize = 2;
// The characters that make a shell expand a word: a variable, a home, a glob or a brace.
const EXPANDS: [char; 6] = ['$', '~', '*', '?', '[', '{'];
// The grep and rg options whose value is the pattern: every operand after them is a file.
const PATTERN_SHORT: &str = "ef";
const PATTERN_LONG: [&str; 2] = ["--regexp", "--file"];

// The options of one view that take a value, which a later word can hold.
struct ValueOptions {
    short: &'static str,
    long: &'static [&'static str],
}

const NO_VALUES: ValueOptions = ValueOptions {
    short: "",
    long: &[],
};
const NL_VALUES: ValueOptions = ValueOptions {
    short: "bdfhilnsvw",
    long: &[],
};
const BAT_VALUES: ValueOptions = ValueOptions {
    short: "lHrm",
    long: &[
        "--language",
        "--highlight-line",
        "--line-range",
        "--map-syntax",
        "--theme",
        "--style",
        "--tabs",
        "--wrap",
        "--color",
        "--decorations",
        "--paging",
        "--terminal-width",
        "--file-name",
    ],
};
const HEAD_VALUES: ValueOptions = ValueOptions {
    short: "nc",
    long: &["--lines", "--bytes"],
};
const GREP_VALUES: ValueOptions = ValueOptions {
    short: "efmABCdD",
    long: &[
        "--regexp",
        "--file",
        "--max-count",
        "--after-context",
        "--before-context",
        "--context",
        "--include",
        "--exclude",
        "--exclude-dir",
        "--label",
        "--devices",
        "--directories",
        "--binary-files",
    ],
};
const RG_VALUES: ValueOptions = ValueOptions {
    short: "efgtTABCmMjEr",
    long: &[
        "--regexp",
        "--file",
        "--glob",
        "--iglob",
        "--type",
        "--type-not",
        "--after-context",
        "--before-context",
        "--context",
        "--max-count",
        "--max-columns",
        "--max-depth",
        "--threads",
        "--encoding",
        "--replace",
        "--sort",
        "--sortr",
    ],
};

// The operands of a view's arguments, and whether an option gave the pattern.
struct Operands<'w> {
    words: Vec<&'w CodePointString>,
    pattern_given: bool,
}

// The file of a read-before-edit view of one file, as written, below the folder of a `cd`
// before it. A doubtful shape reads no file.
pub(crate) fn single_file_read(command: &CodePointString) -> Option<CodePointString> {
    let operators = operators(command)?;
    let words = split_shell_code_points(command).ok()?;
    match operators.as_slice() {
        [] => viewed(&words).cloned(),
        [(AND_AT, operator)] if lossy(operator) == AND => after_cd(&words),
        _ => None,
    }
}

fn viewed(words: &[CodePointString]) -> Option<&CodePointString> {
    let (first, rest) = words.split_first()?;
    match view(posix_name(&lossy(first)))? {
        View::Plain(values) => only(&operands(rest, values).words),
        View::Sed => sed_print(rest),
        View::Search(values) => searched_file(rest, values),
    }
}

// `cd DIR && VIEW`, with DIR a word that the shell does not expand and `&&` a word of its own.
fn after_cd(words: &[CodePointString]) -> Option<CodePointString> {
    let [cd, dir, and, view @ ..] = words else {
        return None;
    };
    let folder = lossy(dir);
    let literal = !folder.is_empty() && !folder.starts_with('-') && !folder.contains(EXPANDS);
    if lossy(cd) != CD || lossy(and) != AND || !literal {
        return None;
    }
    let file = viewed(view)?;
    if lossy(file).starts_with('/') {
        return Some(file.clone());
    }
    let mut below = dir.clone();
    if !folder.ends_with('/') {
        below.push(u32::from('/'));
    }
    for code in file.code_points() {
        below.push(code);
    }
    Some(below)
}

// How a view names its file: its only operand, sed's operand after the range, or a search's
// operand after the pattern.
enum View {
    Plain(&'static ValueOptions),
    Sed,
    Search(&'static ValueOptions),
}

// The views that Claude Code counts as a read before an edit when they run on one file with no
// pipe or redirect.
fn view(program: &str) -> Option<View> {
    let found = match program {
        "cat" => View::Plain(&NO_VALUES),
        "nl" => View::Plain(&NL_VALUES),
        "bat" | "batcat" => View::Plain(&BAT_VALUES),
        "head" | "tail" => View::Plain(&HEAD_VALUES),
        "sed" => View::Sed,
        name if GREP_NAMES.contains(&name) => View::Search(&GREP_VALUES),
        name if RG_NAMES.contains(&name) => View::Search(&RG_VALUES),
        _ => return None,
    };
    Some(found)
}

// Each operator run outside quotes, with its place among the words. Spaces around each run make
// one outside quotes a word of its own, while one inside quotes stays in its word.
fn operators(command: &CodePointString) -> Option<Vec<(usize, CodePointString)>> {
    let is = |code: u32, set: &[char]| char::from_u32(code).is_some_and(|c| set.contains(&c));
    if command.code_points().any(|code| is(code, &LINE_BREAKS)) {
        return None;
    }
    let mut padded = CodePointString::default();
    let mut in_run = false;
    for code in command.code_points() {
        let operator = is(code, &OPERATORS);
        if operator != in_run {
            padded.push(u32::from(' '));
        }
        padded.push(code);
        in_run = operator;
    }
    let words = split_shell_code_points(&padded).ok()?;
    let operators = words.into_iter().enumerate().filter(|(_, word)| {
        word.code_points().next().is_some() && word.code_points().all(|code| is(code, &OPERATORS))
    });
    Some(operators.collect())
}

fn only<'w>(words: &[&'w CodePointString]) -> Option<&'w CodePointString> {
    match words {
        [file] => Some(file),
        _ => None,
    }
}

// A grep or rg of one file: the first operand is the pattern unless an option gave it.
fn searched_file<'w>(
    args: &'w [CodePointString],
    values: &ValueOptions,
) -> Option<&'w CodePointString> {
    let found = operands(args, values);
    let skip = usize::from(!found.pattern_given);
    found.words.get(skip..).and_then(only)
}

// sed -n 'X,Yp' FILE, with X and Y line numbers or Y the last line.
fn sed_print(args: &[CodePointString]) -> Option<&CodePointString> {
    let [quiet, script, file] = args else {
        return None;
    };
    (lossy(quiet) == SED_QUIET && is_print_range(&lossy(script))).then_some(file)
}

fn is_print_range(script: &str) -> bool {
    let Some(range) = script.strip_suffix('p') else {
        return false;
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    match range.split_once(',') {
        Some((start, end)) => digits(start) && (digits(end) || end == "$"),
        None => digits(range),
    }
}

fn operands<'w>(args: &'w [CodePointString], values: &ValueOptions) -> Operands<'w> {
    let mut found = Operands {
        words: Vec::new(),
        pattern_given: false,
    };
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let text = lossy(word);
        if text == END_OF_OPTIONS {
            found.words.extend(words);
            break;
        }
        if text.len() < 2 || !text.starts_with('-') {
            found.words.push(word);
            continue;
        }
        let (takes_next, gives_pattern) = option(&text, values);
        found.pattern_given |= gives_pattern;
        if takes_next {
            words.next();
        }
    }
    found
}

// Whether the option's value is the next word, and whether its value is the pattern.
fn option(text: &str, values: &ValueOptions) -> (bool, bool) {
    if text.starts_with(END_OF_OPTIONS) {
        let (name, attached) = text
            .split_once('=')
            .map_or((text, false), |(name, _)| (name, true));
        let takes_value = values.long.contains(&name);
        return (takes_value && !attached, PATTERN_LONG.contains(&name));
    }
    let letters: Vec<char> = text.chars().skip(1).collect();
    let Some(at) = letters
        .iter()
        .position(|letter| values.short.contains(*letter))
    else {
        return (false, false);
    };
    (at + 1 == letters.len(), PATTERN_SHORT.contains(letters[at]))
}

#[cfg(test)]
mod tests;
