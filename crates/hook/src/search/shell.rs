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
