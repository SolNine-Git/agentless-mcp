//! The tree search in a shell command: the words, the commands between separators, and the grep,
//! rg and find rules. A shape the rules cannot read with certainty is no tree search.

use text::{CodePointString, posix_name, split_shell_code_points};

use super::Suppressed;
use super::stat::{self, Base};
use crate::text::lossy;

const GREP_NAMES: [&str; 3] = ["grep", "egrep", "fgrep"];
// rg walks the working directory by default, so it follows another rule than grep.
const RG_NAMES: [&str; 2] = ["rg", "ripgrep"];
const FIND_NAMES: [&str; 3] = ["find", "fd", "fdfind"];
const RECURSIVE_FLAGS: [&str; 4] = ["-r", "-R", "--recursive", "--dereference-recursive"];
const FIND_SEARCH_FLAGS: [&str; 6] = ["-name", "-iname", "-path", "-ipath", "-regex", "-iregex"];
// A command after a pipe filters the output of the one before it, and reads no repository file.
const PIPE: &str = "|";
const SEPARATORS: [&str; 5] = ["|", "||", "&&", ";", "&"];
const DASH: u32 = '-' as u32;
const RECURSIVE_LETTERS: [u32; 2] = ['r' as u32, 'R' as u32];

// Unbalanced quoting is doubt, not evidence: the command may not even run.
pub(crate) fn broad_search(command: &CodePointString, cwd: &Base<'_>) -> Result<bool, Suppressed> {
    let Ok(words) = split_shell_code_points(command) else {
        return Ok(false);
    };
    for (piped, argv) in commands(&words) {
        if !piped && searches_a_tree(argv, cwd)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn is(word: &CodePointString, names: &[&str]) -> bool {
    word.as_str().is_some_and(|word| names.contains(&word))
}

// Each command between separators, with true when it reads the output of the one before it.
fn commands(words: &[CodePointString]) -> Vec<(bool, &[CodePointString])> {
    let mut found = Vec::new();
    let (mut start, mut piped) = (0, false);
    for (at, word) in words.iter().enumerate() {
        if is(word, &SEPARATORS) {
            found.push((piped, &words[start..at]));
            (start, piped) = (at + 1, is(word, &[PIPE]));
        }
    }
    found.push((piped, &words[start..]));
    found.retain(|(_, argv)| !argv.is_empty());
    found
}

// The program is the last part of the first word. A lone surrogate becomes U+FFFD, which no
// program name holds.
fn searches_a_tree(words: &[CodePointString], cwd: &Base<'_>) -> Result<bool, Suppressed> {
    let Some((first, rest)) = words.split_first() else {
        return Ok(false);
    };
    let first = lossy(first);
    match posix_name(&first) {
        name if GREP_NAMES.contains(&name) => Ok(rest.iter().any(recursive)),
        name if RG_NAMES.contains(&name) => rg_walks_a_tree(rest, cwd),
        name if FIND_NAMES.contains(&name) => {
            Ok(rest.iter().any(|word| is(word, &FIND_SEARCH_FLAGS)))
        }
        _ => Ok(false),
    }
}

// A recursive flag, or a short cluster such as -rn. A long option such as --regexp is no cluster.
fn recursive(word: &CodePointString) -> bool {
    if is(word, &RECURSIVE_FLAGS) {
        return true;
    }
    let codes: Vec<u32> = word.code_points().collect();
    match codes.split_first() {
        Some((&DASH, rest)) if rest.first().is_some_and(|&code| code != DASH) => {
            rest.iter().any(|code| RECURSIVE_LETTERS.contains(code))
        }
        _ => false,
    }
}

// rg walks the working directory unless every path after the pattern is something other than a
// directory. A word that starts with a dash is an option, not a path.
fn rg_walks_a_tree(args: &[CodePointString], cwd: &Base<'_>) -> Result<bool, Suppressed> {
    let operands = args.iter().filter(|arg| !starts_with_dash(arg));
    let mut paths = operands.skip(1).peekable();
    if paths.peek().is_none() {
        return Ok(true);
    }
    for path in paths {
        if stat::found(cwd, path)?.is_some_and(|metadata| metadata.is_dir()) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn starts_with_dash(word: &CodePointString) -> bool {
    word.code_points().next() == Some(DASH)
}

const SED_QUIET: &str = "-n";
// The characters of a pipe, a redirect, a command list and a command substitution.
const OPERATORS: [char; 6] = ['|', '&', ';', '<', '>', '`'];
const LINE_BREAKS: [char; 2] = ['\n', '\r'];
const END_OF_OPTIONS: &str = "--";
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

// The one file that `command` reads when it is a read-before-edit view of one file, as written:
// relative to the command's directory. Any other shape, and a shape the rules cannot read with
// certainty, reads no file.
pub(crate) fn single_file_read(command: &CodePointString) -> Option<CodePointString> {
    if has_operator(command) {
        return None;
    }
    let words = split_shell_code_points(command).ok()?;
    let (first, rest) = words.split_first()?;
    let file = match view(posix_name(&lossy(first)))? {
        View::Plain(values) => only(&operands(rest, values).words),
        View::Sed => sed_print(rest),
        View::Search(values) => searched_file(rest, values),
    };
    file.cloned()
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

// An operator inside quotes is text, so each operator character gets spaces around it before the
// split: one outside quotes then stands as a word of its own, and one inside stays in its word.
fn has_operator(command: &CodePointString) -> bool {
    let is = |code: u32, set: &[char]| char::from_u32(code).is_some_and(|c| set.contains(&c));
    if command.code_points().any(|code| is(code, &LINE_BREAKS)) {
        return true;
    }
    let mut padded = CodePointString::default();
    for code in command.code_points() {
        let operator = is(code, &OPERATORS);
        if operator {
            padded.push(u32::from(' '));
        }
        padded.push(code);
        if operator {
            padded.push(u32::from(' '));
        }
    }
    match split_shell_code_points(&padded) {
        Ok(words) => words.iter().any(|word| {
            word.code_points().next().is_some()
                && word.code_points().all(|code| is(code, &OPERATORS))
        }),
        Err(_) => true,
    }
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
