//! The misses of one trace call: for each missed name, the code files that spell it and do not
//! declare it, as find lists them, and for a name with none, where the target's text occurs. One
//! read of each walked file serves every miss of the call.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use git::RepoFile;
use graph::{FileFacts, joins_name_graph};
use kernel::{Stopped, checkpoint};
use store::FileText;

use super::resolve::{Missed, Resolved};
use super::{Call, MISS_LITERAL_FILES, Miss};
use crate::find::{Spelled, Usage, UsageQuery, spellings, usage, words};
use crate::parallel::{map_in_order, threads};
use crate::scan::Spelling;
use crate::search::{FileHits, Literal, Place, literal_places, searched_text};
use crate::{Error, Noted};

// A missed name whose usage files and text are read: the name, and the target as written.
struct Named {
    name: String,
    literal: String,
}

// What the reads found for one name: its usage files, or the places of its text.
struct Found {
    usage: Option<Usage>,
    literal: Option<(Vec<Place>, usize)>,
}

// The names of a call's misses in card order, read together at the first miss card.
pub(super) struct Misses {
    named: Vec<Named>,
    found: Option<std::vec::IntoIter<Found>>,
}

impl Misses {
    // The first copy of each target that is a missed name with text, in target order: the cards
    // that `card` answers, in the order it answers them.
    pub(super) fn of(targets: &[String], resolved: &[Resolved]) -> Misses {
        let named = (targets.iter().zip(resolved).enumerate())
            .filter(|(at, (target, _))| !targets[..*at].contains(target))
            .filter_map(|(_, (_, resolved))| match resolved {
                Resolved::Missed(Missed::Name { name, literal }) if !literal.trim().is_empty() => {
                    Some(Named {
                        name: name.clone(),
                        literal: literal.clone(),
                    })
                }
                _ => None,
            })
            .collect();
        Misses { named, found: None }
    }

    // Each card gives the warnings that its own find and text search gave: the scan's, then the
    // walk's when the text was searched.
    pub(super) fn card(
        &mut self,
        call: &Call<'_, '_, '_, '_>,
        missed: Missed,
    ) -> Noted<Result<Miss, Error>> {
        let (name, literal) = match missed {
            Missed::Ready(miss) => return Noted::new(Ok(miss)),
            Missed::Name { name, literal } => (name, literal),
        };
        if literal.trim().is_empty() {
            let (usage, literal) = (None, None);
            return Noted::new(Ok(Miss::Nothing {
                name,
                usage,
                literal,
            }));
        }
        let found = match &mut self.found {
            Some(found) => found,
            None => match found(call, &self.named) {
                Ok(found) => self.found.insert(found.into_iter()),
                Err(error) => return Noted::new(Err(error)),
            },
        };
        let next = found.next().expect("one read per missed name");
        let mut diagnostics = call.scan.warnings.clone();
        if next.literal.is_some() {
            diagnostics.extend(call.walk_warnings.iter().cloned());
        }
        let value = Ok(Miss::Nothing {
            name,
            usage: next.usage,
            literal: next.literal,
        });
        Noted { value, diagnostics }
    }
}

// What one read of a scanned code file gives: where it spells each word, and the hits of each
// literal; `None` for a file that can no longer be read.
type CodeRead = Option<(Vec<Option<Spelling>>, Vec<Option<FileHits>>)>;

// The scan's code files are read for the words of every name and the text of every target. Only
// a name with no usage file makes the other walked files read, for its text alone.
fn found(call: &Call<'_, '_, '_, '_>, named: &[Named]) -> Result<Vec<Found>, Error> {
    let queries: Vec<Option<UsageQuery<'_>>> = (named.iter())
        .map(|named| UsageQuery::of(&named.name))
        .collect();
    let listed: Vec<UsageQuery<'_>> = queries.iter().flatten().copied().collect();
    let words = words(&listed);
    let literals: Vec<Result<Literal, Error>> = (named.iter())
        .map(|named| Literal::new(&named.literal))
        .collect();
    let code: Vec<&FileFacts> = (call.scan.files.iter())
        .map(|file| &**file)
        .filter(|file| joins_name_graph(file.language))
        .collect();
    let mut hits: Vec<Vec<FileHits>> = named.iter().map(|_| Vec::new()).collect();
    let spelled = read_code_files(call, &code, &words, &literals, &mut hits)?;
    let views = views(call.scan.files.as_slice(), &spelled);
    let usages: Vec<Option<Usage>> = (queries.iter())
        .map(|query| {
            query
                .as_ref()
                .and_then(|query| usage(&views, &words, query))
        })
        .collect();
    let wanted: Vec<bool> = usages.iter().map(Option::is_none).collect();
    if wanted.contains(&true) {
        read_other_files(call, &code, &literals, &wanted, &mut hits)?;
    }
    let rows = usages.into_iter().zip(literals).zip(hits);
    rows.map(|((usage, literal), hits)| match usage {
        Some(usage) => Ok(Found {
            usage: Some(usage),
            literal: None,
        }),
        None => literal.map(|_| Found {
            usage: None,
            literal: Some(literal_places(hits, MISS_LITERAL_FILES)),
        }),
    })
    .collect()
}

// Where each code file spells the words, or `None` for a file that can no longer be read; the
// hits of each literal go to `hits`.
fn read_code_files(
    call: &Call<'_, '_, '_, '_>,
    code: &[&FileFacts],
    words: &[&str],
    literals: &[Result<Literal, Error>],
    hits: &mut [Vec<FileHits>],
) -> Result<Vec<Option<Vec<Option<Spelling>>>>, Stopped> {
    let (root, deadline) = (&call.reading.repo.root, call.reading.deadline);
    let read: Vec<Result<CodeRead, Stopped>> = map_in_order(code, threads(), |file| {
        checkpoint(deadline)?;
        Ok(read_code(root, file, words, literals))
    });
    let mut spelled = Vec::with_capacity(read.len());
    for file in read {
        let Some((spellings, file_hits)) = file? else {
            spelled.push(None);
            continue;
        };
        held(hits, file_hits);
        spelled.push(Some(spellings));
    }
    Ok(spelled)
}

// The hits of each wanted literal in the walked files that the scan did not read as code.
fn read_other_files(
    call: &Call<'_, '_, '_, '_>,
    code: &[&FileFacts],
    literals: &[Result<Literal, Error>],
    wanted: &[bool],
    hits: &mut [Vec<FileHits>],
) -> Result<(), Stopped> {
    let (root, deadline) = (&call.reading.repo.root, call.reading.deadline);
    let read: HashSet<&str> = code.iter().map(|file| file.path.as_str()).collect();
    let others: Vec<&RepoFile> = (call.files.iter())
        .filter(|file| !read.contains(file.path.as_str()))
        .collect();
    let other_hits: Vec<Result<Vec<Option<FileHits>>, Stopped>> =
        map_in_order(&others, threads(), |file| {
            checkpoint(deadline)?;
            Ok(read_other(root, file, literals, wanted))
        });
    for file_hits in other_hits {
        held(hits, file_hits?);
    }
    Ok(())
}

fn held(hits: &mut [Vec<FileHits>], file_hits: Vec<Option<FileHits>>) {
    for (held, hit) in hits.iter_mut().zip(file_hits) {
        held.extend(hit);
    }
}

// Each scanned file as the usage rule reads it, in scan order. A code file that could not be read
// again is left out, as the scan of a find of its own leaves it out.
fn views<'f>(
    files: &'f [Arc<FileFacts>],
    spelled: &'f [Option<Vec<Option<Spelling>>>],
) -> Vec<Spelled<'f>> {
    let mut code = spelled.iter();
    (files.iter())
        .filter_map(|file| {
            let spellings: &[Option<Spelling>] = if joins_name_graph(file.language) {
                code.next().expect("one read per code file").as_deref()?
            } else {
                &[]
            };
            Some(Spelled {
                path: &file.path,
                symbols: &file.facts.symbols,
                spellings,
            })
        })
        .collect()
}

fn read_code(
    root: &Path,
    file: &FileFacts,
    words: &[&str],
    literals: &[Result<Literal, Error>],
) -> CodeRead {
    let bytes = io::read_bounded(&root.join(&file.path)).ok()?;
    let text = FileText::decode(file.path.clone(), file.language, &bytes);
    let searched = searched_text(&bytes);
    let hits = (literals.iter())
        .map(|literal| {
            literal
                .as_ref()
                .ok()?
                .hits(&file.path, searched.as_deref()?)
        })
        .collect();
    Some((spellings(&text, words), hits))
}

fn read_other(
    root: &Path,
    file: &RepoFile,
    literals: &[Result<Literal, Error>],
    wanted: &[bool],
) -> Vec<Option<FileHits>> {
    let bytes = io::read_bounded(&root.join(&file.path)).ok();
    let text = bytes.as_deref().and_then(searched_text);
    (literals.iter().zip(wanted))
        .map(|(literal, wanted)| {
            let literal = literal.as_ref().ok().filter(|_| *wanted)?;
            literal.hits(&file.path, text.as_deref()?)
        })
        .collect()
}
