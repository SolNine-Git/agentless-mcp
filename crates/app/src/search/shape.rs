//! The shape of a search answer: noise files as counts, source files first, the limit on match
//! lines, one copy of a text that repeats, and the symbol that holds each shown line.

use std::collections::BTreeMap;

use facts::{StableId, Symbol};
use graph::enclosing_symbol;
use syntax::Language;

use super::lines::FileHits;
use super::{
    Block, FileCount, Line, Matches, Noise, NoiseClass, Owner, Place, REPEAT_PLACES, Repeat,
};

// The shown lines by file, in answer order, and the rest of the answer with no groups yet.
pub(super) struct Shaped {
    pub(super) groups: Vec<(String, Vec<Line>)>,
    pub(super) matches: Matches,
}

// `named` is the file that the request's path names; its class does not hide it. The repeats are
// found only with no context, where every shown line is a match line.
pub(super) fn shaped(
    hits: Vec<FileHits>,
    named: Option<&str>,
    limit: usize,
    context: usize,
) -> Shaped {
    let matches = hits.iter().map(|file| file.matches).sum();
    let files = hits.len();
    let (noisy, plain): (Vec<FileHits>, Vec<FileHits>) = (hits.into_iter())
        .partition(|file| file.class.is_some() && named != Some(file.path.as_str()));
    let limited = limited(ordered(plain), limit, context);
    let (groups, repeats) = if context == 0 {
        repeated(limited.groups)
    } else {
        (limited.groups, Vec::new())
    };
    Shaped {
        groups,
        matches: Matches {
            matches,
            files,
            searched: 0,
            groups: Vec::new(),
            repeats,
            shown: limited.shown,
            rest: limited.rest,
            noise: noise(&noisy),
            skipped: Vec::new(),
        },
    }
}

// Source files first, each part in path order.
fn ordered(mut files: Vec<FileHits>) -> Vec<FileHits> {
    files.sort_by(|a, b| (!a.source, &a.path).cmp(&(!b.source, &b.path)));
    files
}

// The groups that the limit kept, how many match lines they hold, and what it left out.
struct Limited {
    groups: Vec<(String, Vec<Line>)>,
    shown: usize,
    rest: Vec<FileCount>,
}

// The limit is spent in answer order. A file that it cuts keeps the context lines after its last
// kept match, and the rest of its matches are counted.
fn limited(files: Vec<FileHits>, limit: usize, context: usize) -> Limited {
    let mut kept = Limited {
        groups: Vec::new(),
        shown: 0,
        rest: Vec::new(),
    };
    for file in files {
        let room = limit - kept.shown;
        let taken = file.matches.min(room);
        if taken > 0 {
            kept.groups
                .push((file.path.clone(), first_matches(file.lines, taken, context)));
        }
        kept.shown += taken;
        if taken < file.matches {
            kept.rest.push(FileCount {
                path: file.path,
                count: file.matches - taken,
            });
        }
    }
    kept
}

// The lines through the `taken`-th match line and the `context` lines after it, short of the next
// match line.
fn first_matches(lines: Vec<Line>, taken: usize, context: usize) -> Vec<Line> {
    let mut seen = 0;
    let mut last = None;
    let mut kept = Vec::new();
    for line in lines {
        if line.matched {
            if seen == taken {
                break;
            }
            seen += 1;
            last = (seen == taken).then_some(line.number);
        } else if last.is_some_and(|last| after(last, line.number) > context) {
            break;
        }
        kept.push(line);
    }
    kept
}

// How many lines `line` is after `last`.
fn after(last: u32, line: u32) -> usize {
    usize::try_from(line.saturating_sub(last)).unwrap_or(usize::MAX)
}

// Each text that `REPEAT_PLACES` or more shown lines hold leaves its groups for one repeat, in
// the order the answer first reaches it. A group left with no line goes.
fn repeated(groups: Vec<(String, Vec<Line>)>) -> (Vec<(String, Vec<Line>)>, Vec<Repeat>) {
    let mut places: BTreeMap<&str, Vec<Place>> = BTreeMap::new();
    let mut order: Vec<&str> = Vec::new();
    for (path, lines) in &groups {
        for line in lines {
            let seen = places.entry(line.text.as_str()).or_default();
            if seen.is_empty() {
                order.push(line.text.as_str());
            }
            seen.push(Place {
                path: path.clone(),
                line: line.number,
            });
        }
    }
    let repeats: Vec<Repeat> = (order.into_iter())
        .filter(|text| places[text].len() >= REPEAT_PLACES)
        .map(|text| Repeat {
            text: text.to_owned(),
            places: places[text].clone(),
        })
        .collect();
    let repeat = |line: &Line| repeats.iter().any(|repeat| repeat.text == line.text);
    let kept = (groups.into_iter())
        .filter_map(|(path, lines)| {
            let lines: Vec<Line> = lines.into_iter().filter(|line| !repeat(line)).collect();
            (!lines.is_empty()).then_some((path, lines))
        })
        .collect();
    (kept, repeats)
}

// One count per class, in class order, each class's files in path order.
fn noise(files: &[FileHits]) -> Vec<Noise> {
    let mut classes: BTreeMap<NoiseClass, Noise> = BTreeMap::new();
    for file in files {
        let Some(class) = file.class else {
            continue;
        };
        let entry = classes.entry(class).or_insert_with(|| Noise {
            class,
            matches: 0,
            files: Vec::new(),
        });
        entry.matches += file.matches;
        entry.files.push(FileCount {
            path: file.path.clone(),
            count: file.matches,
        });
    }
    let mut noise: Vec<Noise> = classes.into_values().collect();
    for class in &mut noise {
        class.files.sort_by(|a, b| a.path.cmp(&b.path));
    }
    noise
}

// The lines in blocks by the innermost symbol that spans them, from `symbols` of the file at
// `path`. With no symbols, every line is outside every symbol.
pub(super) fn owner_blocks(
    path: &str,
    lines: Vec<Line>,
    symbols: Option<&Vec<Symbol>>,
) -> Vec<Block> {
    let language = Language::from_path(path);
    let mut blocks: Vec<Block> = Vec::new();
    for line in lines {
        let owner = (symbols.zip(language))
            .and_then(|(symbols, language)| owner(language, path, symbols, line.number));
        match blocks.last_mut() {
            Some(block) if block.owner == owner => block.lines.push(line),
            _ => blocks.push(Block {
                owner,
                lines: vec![line],
            }),
        }
    }
    blocks
}

fn owner(language: Language, path: &str, symbols: &[Symbol], line: u32) -> Option<Owner> {
    let symbol = enclosing_symbol(symbols, line)?;
    Some(Owner {
        qualname: symbol.qualname(),
        stable_id: StableId::of(language, path, symbol).to_string(),
        start_line: symbol.line,
        end_line: symbol.end_line,
    })
}

// The first match line of each of the first `cap` files, in answer order.
pub(super) fn first_places(files: Vec<FileHits>, cap: usize) -> Vec<Place> {
    let first = |file: &FileHits| {
        file.lines
            .iter()
            .find(|line| line.matched)
            .map(|line| line.number)
    };
    (ordered(files).iter())
        .filter_map(|file| {
            first(file).map(|line| Place {
                path: file.path.clone(),
                line,
            })
        })
        .take(cap)
        .collect()
}
