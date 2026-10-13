//! The argument counts of positional actions, matched against the pattern of one command line as
//! argparse matches its nargs patterns: greedy, with backtracking, anchored at the start only.
//! The pattern holds `A` for a value, `O` for an option and `-` for the `--` separator.

use app::ops::Arity;

// The arity of a positional action, or of the subcommand: one name, then everything after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Nargs {
    Arity(Arity),
    Parser,
}

#[derive(Clone, Copy)]
enum Token {
    // Any run of the listed pattern characters, the longest first.
    Star(&'static [u8]),
    // The character once if it is there, else nothing.
    Optional(u8),
    // The character once.
    One(u8),
}

fn tokens(nargs: Nargs) -> &'static [Token] {
    const DASHES: Token = Token::Star(b"-");
    match nargs {
        Nargs::Arity(Arity::One) => &[DASHES, Token::One(b'A'), DASHES],
        Nargs::Arity(Arity::Optional) => &[DASHES, Token::Optional(b'A'), DASHES],
        Nargs::Arity(Arity::Any) => &[DASHES, Token::Star(b"A-")],
        Nargs::Arity(Arity::OneOrMore) => &[DASHES, Token::One(b'A'), Token::Star(b"A-")],
        Nargs::Parser => &[DASHES, Token::One(b'A'), Token::Star(b"-AO")],
    }
}

// The counts of the longest prefix of `nargs` whose patterns match the start of `pattern`. When an
// option follows the match, the trailing zero counts go, so those positionals wait for a later run.
pub(crate) fn match_partial(nargs: &[Nargs], pattern: &[u8]) -> Vec<usize> {
    let found = (1..=nargs.len())
        .rev()
        .find_map(|take| matched(&nargs[..take], pattern, 0));
    let Some(mut counts) = found else {
        return Vec::new();
    };
    let end: usize = counts.iter().sum();
    if pattern.get(end) == Some(&b'O') {
        while counts.last() == Some(&0) {
            counts.pop();
        }
    }
    counts
}

// True when the arguments of a positional hold the `--` that the parse strips from them: a
// subcommand strips a leading one, and every other positional strips the first one it holds.
pub(crate) fn strips_separator(nargs: Nargs, pattern: &[u8]) -> bool {
    match nargs {
        Nargs::Parser => pattern.first() == Some(&b'-'),
        Nargs::Arity(_) => pattern.contains(&b'-'),
    }
}

// The first match in backtracking order: each pattern's longest candidate first.
fn matched(nargs: &[Nargs], pattern: &[u8], at: usize) -> Option<Vec<usize>> {
    let Some((first, rest)) = nargs.split_first() else {
        return Some(Vec::new());
    };
    let mut ends = Vec::new();
    ends_of(tokens(*first), pattern, at, &mut ends);
    ends.into_iter().find_map(|end| {
        let mut counts = matched(rest, pattern, end)?;
        counts.insert(0, end - at);
        Some(counts)
    })
}

fn ends_of(tokens: &[Token], pattern: &[u8], at: usize, ends: &mut Vec<usize>) {
    let Some((token, rest)) = tokens.split_first() else {
        ends.push(at);
        return;
    };
    match *token {
        Token::Star(set) => {
            let run = pattern[at..].iter().take_while(|c| set.contains(c)).count();
            for taken in (0..=run).rev() {
                ends_of(rest, pattern, at + taken, ends);
            }
        }
        Token::Optional(c) => {
            if pattern.get(at) == Some(&c) {
                ends_of(rest, pattern, at + 1, ends);
            }
            ends_of(rest, pattern, at, ends);
        }
        Token::One(c) => {
            if pattern.get(at) == Some(&c) {
                ends_of(rest, pattern, at + 1, ends);
            }
        }
    }
}
