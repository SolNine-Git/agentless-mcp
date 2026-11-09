//! The diff card of a `diff:<rev>` target: the merge base of the revision with HEAD, each source
//! file that differs from it in the working tree, and the symbols that were added, removed,
//! changed or moved, paired by the matcher that the pair checks share. It reads no graph.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use checks::{Match, Matchable, Pairing, pair_units};
use facts::{Symbol, is_data_language};
use git::{Change, GitOutcome, Side};
use kernel::Key;
use prompts::messages::{
    HISTORY_GIT_FAILED, TRACE_DIFF_NO_BASE, TRACE_DIFF_NO_REV, TRACE_DIFF_REV, TRACE_DIFF_UNREAD,
};
use sha2::{Digest, Sha256};
use store::FileText;
use syntax::Language;

use super::rev::named;
use super::{
    Card, CardKind, DIFF_FOLDER_DEPTH, DIFF_FOLDERS, DIFF_ROWS, DiffCounts, DiffFile, DiffFiles,
    DiffFolder, DiffMoved, DiffRow, DiffSection, DiffSymbols, DiffTrace, FileStatus, LISTING_LIMIT,
    MovedRow, Outcome, Section, TraceRequest, Traced, answered,
};
use crate::revision::{blob_from, changes, worktree_read};
use crate::{Error, Noted, Reading, Services, SymbolCard, is_test_path, parallel, symbol_card};

const PREFIX: &str = "diff:";
// Each base side is one git process. More than four at a time read no faster.
const MAX_BLOB_READS: usize = 4;

// A `diff:<rev>` or `diff:<rev>:<path>` target: git refuses ":" in a ref name, so the next ":"
// starts the path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DiffTarget {
    rev: String,
    scope: Option<String>,
}

impl DiffTarget {
    pub(super) fn parse(target: &str) -> Option<DiffTarget> {
        let rest = target.strip_prefix(PREFIX)?;
        let (rev, path) = match rest.split_once(':') {
            Some((rev, path)) => (rev, Some(path)),
            None => (rest, None),
        };
        let scope = path
            .map(|path| path.trim_start_matches("./").trim_end_matches('/'))
            .filter(|path| !path.is_empty())
            .map(str::to_owned);
        let rev = rev.to_owned();
        Some(DiffTarget { rev, scope })
    }

    // The folder levels above the rows that the card rolls its files up to.
    fn depth(&self) -> usize {
        self.scope
            .as_ref()
            .map_or(0, |scope| scope.split('/').count())
    }
}

// A diff compares its revision with the working tree, so a second revision is refused in v1.
pub(super) fn refused_with_rev(targets: &[String]) -> Option<Error> {
    let target = targets
        .iter()
        .find_map(|target| DiffTarget::parse(target))?;
    Some(refused(TRACE_DIFF_REV.render(&target.rev)))
}

// A call whose every target is a diff target answers with no walk and no scan; `None` sends any
// other call to the scan, which answers its diff targets as it resolves them.
pub(super) fn alone(
    services: &Services,
    reading: &Reading<'_>,
    request: &TraceRequest,
) -> Option<Noted<Result<Vec<Traced>, Error>>> {
    let targets: Vec<DiffTarget> = (request.targets.iter())
        .map(|target| DiffTarget::parse(target))
        .collect::<Option<_>>()?;
    Some(answered(&request.targets, |at| {
        let kind = CardKind::Diff;
        match request.more.filter(|more| !kind.sections().contains(more)) {
            Some(section) => Noted::new(Ok(Outcome::NoSection { section, kind })),
            None => diff_card(services, reading, &targets[at], request.more).map_ok(Outcome::Card),
        }
    }))
}

// The warnings name each side that was not read.
pub(super) fn diff_card(
    services: &Services,
    reading: &Reading<'_>,
    target: &DiffTarget,
    more: Option<Section>,
) -> Noted<Result<Card, Error>> {
    let root = reading.repo.root.as_path();
    let (base, commits) = match merge_base(services, root, &target.rev) {
        Ok(found) => found,
        Err(error) => return Noted::new(Err(error)),
    };
    let listed = match changes(&services.git, root, &base) {
        Ok(Ok(listed)) => listed,
        Ok(Err(gap)) => return Noted::new(Err(refused(gap.finding.message))),
        Err(error) => return Noted::new(Err(error)),
    };
    // Every changed file is read, so a body that moved across the scope's edge still pairs.
    let read = read_files(services, reading, &base, file_specs(&listed));
    let compared = Compared::of(&read.value, target.scope.as_deref());
    let shape = Shape::of(target, more, &compared.shown(&read.value));
    let card = DiffTrace {
        rev: target.rev.clone(),
        scope: target.scope.clone(),
        base,
        commits,
        full: shape.full,
        files: compared.files(&read.value, shape, more == Some(Section::Tree)),
        symbols: (more != Some(Section::Tree)).then(|| compared.symbols(shape)),
    };
    Noted {
        value: Ok(Card::Diff(Box::new(card))),
        diagnostics: read.diagnostics,
    }
}

fn refused(message: String) -> Error {
    kernel::Error::OperationFailed(message).into()
}

// The merge base of the commit that `rev` names with HEAD, and the commits HEAD holds past it.
fn merge_base(services: &Services, root: &Path, rev: &str) -> Result<(String, usize), Error> {
    if rev.is_empty() {
        return Err(refused(TRACE_DIFF_NO_REV.to_owned()));
    }
    let commit = named(services, root, rev)?.commit;
    let git = &services.git;
    let base = printed(git.merge_base(root, commit.as_str()))
        .and_then(|stdout| git::parse_commit(&stdout).map_err(|malformed| malformed.to_string()))
        .map_err(|note| refused(TRACE_DIFF_NO_BASE.render((rev, &note))))?;
    let counted = printed(git.commits_since(root, &base)).and_then(|stdout| {
        let text = String::from_utf8_lossy(&stdout);
        text.trim()
            .parse::<usize>()
            .map_err(|error| error.to_string())
    });
    let commits = counted.map_err(|note| refused(HISTORY_GIT_FAILED.render(&note)))?;
    Ok((base, commits))
}

fn printed(outcome: GitOutcome) -> Result<Vec<u8>, String> {
    match outcome {
        GitOutcome::Printed(stdout) => Ok(stdout),
        GitOutcome::Capped(_) => Err(git::Malformed::NotOneObjectName.to_string()),
        GitOutcome::Failed(failure) => Err(failure.to_string()),
        GitOutcome::Unfinished(unfinished) => Err(unfinished.to_string()),
    }
}

// One changed path that a grammar reads on one side or both: its path in the working tree, and
// its path and language in the base and its language in the working tree.
struct FileSpec {
    path: String,
    old: Option<(String, Language)>,
    new: Option<Language>,
}

impl FileSpec {
    fn within(&self, scope: &str) -> bool {
        let under = |path: &str| {
            path == scope || (path.strip_prefix(scope)).is_some_and(|rest| rest.starts_with('/'))
        };
        under(&self.path) || self.old.as_ref().is_some_and(|(path, _)| under(path))
    }

    fn status(&self) -> FileStatus {
        match (&self.old, self.new) {
            (None, _) => FileStatus::Added,
            (Some(_), None) => FileStatus::Removed,
            (Some((old, _)), Some(_)) if *old != self.path => FileStatus::Renamed(old.clone()),
            (Some(_), Some(_)) => FileStatus::Changed,
        }
    }

    fn is_data(&self) -> bool {
        let language = self
            .new
            .or(self.old.as_ref().map(|(_, language)| *language));
        language.is_some_and(is_data_language)
    }
}

// A side counts when it is a regular file that a grammar reads. A change of language is a
// removal and an addition; a copy keeps its source, so only its new side counts.
fn file_specs(changes: &[Change]) -> Vec<FileSpec> {
    let read = |side: Side, path: &str| (side == Side::File).then(|| Language::from_path(path));
    let mut specs: BTreeMap<String, FileSpec> = BTreeMap::new();
    for change in changes {
        let old = (change.status != 'C')
            .then(|| read(change.old, &change.old_path).flatten())
            .flatten();
        let new = read(change.new, &change.path).flatten();
        let parts = match (old, new) {
            (Some(old), Some(new)) if old != new => vec![
                (&change.old_path, Some((change.old_path.clone(), old)), None),
                (&change.path, None, Some(new)),
            ],
            (None, None) => Vec::new(),
            (old, new) => {
                let path = if new.is_some() {
                    &change.path
                } else {
                    &change.old_path
                };
                let old = old.map(|language| (change.old_path.clone(), language));
                vec![(path, old, new)]
            }
        };
        // A deletion and an addition at one path, as `git rm --cached` leaves them, join.
        for (path, old, new) in parts {
            let spec = specs.entry(path.clone()).or_insert(FileSpec {
                path: path.clone(),
                old: None,
                new: None,
            });
            spec.old = spec.old.take().or(old);
            spec.new = spec.new.or(new);
        }
    }
    specs.into_values().collect()
}

// One file's two sides as the symbols that the matcher reads.
struct FileSides {
    spec: FileSpec,
    old: Vec<Unit>,
    new: Vec<Unit>,
}

// A symbol's card, its qualified name, and the digest of its span text with its name masked.
struct Unit {
    card: SymbolCard,
    qualname: String,
    body: Key,
}

impl Matchable for Unit {
    type Body = Key;
    type Rank = u32;

    fn body_key(&self) -> Key {
        self.body
    }

    fn qualname(&self) -> &str {
        &self.qualname
    }

    fn rank(&self) -> u32 {
        self.card.start_line
    }
}

// Reads and parses both sides of each file, four files at a time. A side that git, the disk or
// the parse does not give has no symbols, and a warning names it.
fn read_files(
    services: &Services,
    reading: &Reading<'_>,
    base: &str,
    specs: Vec<FileSpec>,
) -> Noted<Vec<FileSides>> {
    let root = reading.repo.root.as_path();
    let threads = parallel::threads().min(MAX_BLOB_READS);
    let read = parallel::map_in_order(&specs, threads, |spec| {
        let old = (spec.old.as_ref()).map(|(path, language)| {
            let bytes = blob_from(services.git.blob(root, base, path));
            units(reading, (path, *language), bytes)
        });
        let new = spec.new.map(|language| {
            let bytes = worktree_read(root, &spec.path).unwrap_or_else(|gap| {
                let message = gap.finding.message;
                Err(message)
            });
            units(reading, (&spec.path, language), bytes)
        });
        (old.unwrap_or_default(), new.unwrap_or_default())
    });
    let mut noted = Noted::new(Vec::new());
    for (spec, ((old, old_unread), (new, new_unread))) in specs.into_iter().zip(read) {
        noted
            .diagnostics
            .extend(old_unread.into_iter().chain(new_unread));
        noted.value.push(FileSides { spec, old, new });
    }
    noted
}

fn units(
    reading: &Reading<'_>,
    (path, language): (&str, Language),
    bytes: Result<Vec<u8>, String>,
) -> (Vec<Unit>, Vec<String>) {
    let unread = |reason: &str| TRACE_DIFF_UNREAD.render((path, reason));
    let bytes = match bytes {
        Ok(bytes) if git::looks_binary(&bytes) => return (Vec::new(), vec![unread("binary")]),
        Ok(bytes) => bytes,
        Err(reason) => return (Vec::new(), vec![unread(&reason)]),
    };
    let file = FileText::decode(path.to_owned(), language, &bytes);
    let served = reading.facts.symbols(&file, reading.deadline);
    let mut warnings: Vec<String> = served.warning.iter().map(ToString::to_string).collect();
    let symbols = match served.value {
        Ok(symbols) => symbols,
        Err(unparsable) => {
            warnings.push(unread(&unparsable.to_string()));
            return (Vec::new(), warnings);
        }
    };
    let lines: Vec<&str> = file.text.lines().collect();
    let units = (symbols.iter())
        .map(|symbol| Unit {
            card: symbol_card(language, path, symbol, String::new()),
            qualname: symbol.qualname(),
            body: body_key(&lines, symbol),
        })
        .collect();
    (units, warnings)
}

// Each line is trimmed, so a change of indent alone keeps the body.
fn body_key(lines: &[&str], symbol: &Symbol) -> Key {
    let first = usize::try_from(symbol.line.saturating_sub(1)).unwrap_or(usize::MAX);
    let last = (usize::try_from(symbol.end_line).unwrap_or(usize::MAX)).min(lines.len());
    let span = lines.get(first..last).unwrap_or_default();
    let text: Vec<&str> = span.iter().map(|line| line.trim()).collect();
    let masked = text.join("\n").replacen(&symbol.name, "\0", 1);
    Key::new(Sha256::digest(masked.as_bytes()).into())
}

// What the card lists: the cap of each section, the order of its rows, and the folder levels.
#[derive(Clone, Copy)]
struct Shape {
    full: bool,
    line_order: bool,
    levels: usize,
}

impl Shape {
    // A scoped file lists each of its rows in line order; `more` lists each row of the card.
    fn of(target: &DiffTarget, more: Option<Section>, shown: &[&FileSides]) -> Shape {
        let scope = target.scope.as_deref();
        let file = matches!(shown, [only] if scope == Some(only.spec.path.as_str()));
        Shape {
            full: more.is_some() || file,
            line_order: file,
            levels: target.depth() + DIFF_FOLDER_DEPTH,
        }
    }

    fn rows(self) -> usize {
        if self.full { LISTING_LIMIT } else { DIFF_ROWS }
    }
}

// Each symbol as the matcher paired it, with the index of its file. The card shows the files
// under its scope, and counts their symbols, but a data file's only under a scoped target.
struct Compared {
    in_scope: Vec<bool>,
    counted: Vec<bool>,
    changed: Vec<(usize, DiffRow)>,
    added: Vec<(usize, DiffRow)>,
    removed: Vec<(usize, DiffRow)>,
    moved: Vec<MovedRow>,
}

impl Compared {
    fn of(files: &[FileSides], scope: Option<&str>) -> Compared {
        let sides: Vec<(&[Unit], &[Unit])> = (files.iter())
            .map(|file| (file.old.as_slice(), file.new.as_slice()))
            .collect();
        let in_scope: Vec<bool> = (files.iter())
            .map(|file| scope.is_none_or(|scope| file.spec.within(scope)))
            .collect();
        let counted = (files.iter().zip(&in_scope))
            .map(|(file, shown)| *shown && (scope.is_some() || !file.spec.is_data()))
            .collect();
        let mut compared = Compared {
            in_scope,
            counted,
            changed: Vec::new(),
            added: Vec::new(),
            removed: Vec::new(),
            moved: Vec::new(),
        };
        let mut paired: HashSet<(usize, usize)> = HashSet::new();
        for pairing in pair_units(&sides) {
            paired.extend(pairing.old);
            compared.pairing(files, pairing);
        }
        let counted = compared.counted.clone();
        for (at, file) in files.iter().enumerate().filter(|(at, _)| counted[*at]) {
            let gone = (0..file.old.len()).filter(|index| !paired.contains(&(at, *index)));
            let rows = gone.map(|index| (at, row(&file.old[index], None)));
            compared.removed.extend(rows);
        }
        compared
    }

    // A body in another file or under another name moved; one name with other text changed.
    fn pairing(&mut self, files: &[FileSides], pairing: Pairing) {
        let (at, new) = (pairing.new.0, &files[pairing.new.0].new[pairing.new.1]);
        let old = pairing
            .old
            .map(|(file, index)| (file, &files[file].old[index]));
        let counted = |file: usize| self.counted[file];
        let moved_out = pairing.via == Match::Body && old.is_some_and(|(file, _)| counted(file));
        if !(counted(at) || moved_out) {
            return;
        }
        match (pairing.via, old) {
            (Match::New, _) | (_, None) => self.added.push((at, row(new, None))),
            (Match::Body, Some((file, old))) => {
                if file != at || old.qualname != new.qualname {
                    let (from, to) = (old.card.clone(), new.card.clone());
                    self.moved.push(MovedRow { from, to });
                }
            }
            (Match::Name | Match::Collision, Some((_, old))) => {
                let base = Some((old.card.start_line, old.card.end_line));
                self.changed.push((at, row(new, base)));
            }
        }
    }

    // The counts of each file's symbols, by its index.
    fn counts(&self, files: usize) -> Vec<DiffCounts> {
        let mut counts = vec![DiffCounts::default(); files];
        for (at, _) in &self.added {
            counts[*at].added += 1;
        }
        for (at, _) in &self.removed {
            counts[*at].removed += 1;
        }
        for (at, _) in &self.changed {
            counts[*at].changed += 1;
        }
        counts
    }

    fn shown<'f>(&self, files: &'f [FileSides]) -> Vec<&'f FileSides> {
        (files.iter().zip(&self.in_scope))
            .filter_map(|(file, shown)| shown.then_some(file))
            .collect()
    }

    fn files(&self, files: &[FileSides], shape: Shape, each: bool) -> DiffFiles {
        let counts = self.counts(files.len());
        let rows: Vec<DiffFile> = (files.iter().zip(&counts).zip(&self.in_scope))
            .filter_map(|(file, shown)| shown.then_some(file))
            .map(|(file, counts)| DiffFile {
                path: shown_path(&file.spec).to_owned(),
                status: file.spec.status(),
                symbols: *counts,
            })
            .collect();
        let (folders, folders_total) = folders(&rows, shape.levels);
        let count =
            |wanted: fn(&FileStatus) -> bool| rows.iter().filter(|row| wanted(&row.status)).count();
        DiffFiles {
            total: rows.len(),
            added: count(|status| *status == FileStatus::Added),
            removed: count(|status| *status == FileStatus::Removed),
            changed: count(|status| *status == FileStatus::Changed),
            renamed: count(|status| matches!(status, FileStatus::Renamed(_))),
            folders,
            folders_total,
            rows: each.then(|| rows.into_iter().take(LISTING_LIMIT).collect()),
        }
    }

    fn symbols(&self, shape: Shape) -> DiffSymbols {
        let mut moved: Vec<MovedRow> = self.moved.clone();
        moved.sort_by_key(|row| order(&row.to, shape));
        let total = moved.len();
        moved.truncate(shape.rows());
        DiffSymbols {
            changed: section(&self.changed, shape),
            added: section(&self.added, shape),
            removed: section(&self.removed, shape),
            moved: DiffMoved { total, rows: moved },
        }
    }
}

// A removed file is at its base path.
fn shown_path(spec: &FileSpec) -> &str {
    match (&spec.old, spec.new) {
        (Some((old, _)), None) => old,
        _ => &spec.path,
    }
}

fn row(unit: &Unit, base: Option<(u32, u32)>) -> DiffRow {
    DiffRow {
        card: unit.card.clone(),
        base,
    }
}

fn section(rows: &[(usize, DiffRow)], shape: Shape) -> DiffSection {
    let files: HashSet<usize> = rows.iter().map(|(at, _)| *at).collect();
    let mut listed: Vec<DiffRow> = rows.iter().map(|(_, row)| row.clone()).collect();
    listed.sort_by_key(|row| order(&row.card, shape));
    let tests = (listed.iter())
        .filter(|row| is_test_path(&row.card.path))
        .count();
    let total = listed.len();
    listed.truncate(shape.rows());
    DiffSection {
        total,
        files: files.len(),
        tests,
        rows: listed,
    }
}

// Rows outside test paths first, then the longest span; a scoped file keeps line order.
fn order(card: &SymbolCard, shape: Shape) -> (bool, Reverse<u32>, String, u32) {
    if shape.line_order {
        return (false, Reverse(0), card.path.clone(), card.start_line);
    }
    let span = card.end_line.saturating_sub(card.start_line);
    let test = is_test_path(&card.path);
    (test, Reverse(span), card.path.clone(), card.start_line)
}

// Each row's folder is the first `levels` folders of its path; most files first, then path
// order, cut at the cap, with the count before the cut.
fn folders(rows: &[DiffFile], levels: usize) -> (Vec<DiffFolder>, usize) {
    let mut by_path: BTreeMap<String, DiffFolder> = BTreeMap::new();
    for row in rows {
        let parts: Vec<&str> = row.path.split('/').collect();
        let path = parts[..(parts.len() - 1).min(levels)].join("/");
        let folder = by_path.entry(path.clone()).or_insert(DiffFolder {
            path,
            files: 0,
            symbols: DiffCounts::default(),
        });
        folder.files += 1;
        folder.symbols.added += row.symbols.added;
        folder.symbols.removed += row.symbols.removed;
        folder.symbols.changed += row.symbols.changed;
    }
    let total = by_path.len();
    let mut folders: Vec<DiffFolder> = by_path.into_values().collect();
    folders.sort_by_key(|folder| Reverse(folder.files));
    folders.truncate(DIFF_FOLDERS);
    (folders, total)
}

#[cfg(test)]
mod tests;
