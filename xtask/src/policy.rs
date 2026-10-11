use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;
use toml::Table;

use crate::{Failure, repo, verdict};

const TRAILER: &str = "Policy-Change";
const FILES: [(&str, Format); 6] = [
    (".agentless-mcp.json", Format::Json),
    ("deny.toml", Format::Toml),
    ("rustfmt.toml", Format::Toml),
    ("rust-toolchain.toml", Format::Toml),
    (".config/nextest.toml", Format::Toml),
    ("crates/layers.toml", Format::Toml),
];
const MANIFEST: &str = "Cargo.toml";
const MANIFEST_TABLES: [&[&str]; 3] = [&["workspace", "lints"], &["patch"], &["replace"]];
const CLIPPY_NAMES: [&str; 2] = ["clippy.toml", ".clippy.toml"];

#[derive(Clone, Copy)]
enum Format {
    Json,
    Toml,
    Manifest,
}

struct Item {
    path: String,
    format: Format,
}

struct Commit {
    parents: Vec<String>,
    trailers: Vec<String>,
}

struct Step {
    commit: String,
    approved: bool,
    value: Option<Value>,
    parents: Vec<Option<Value>>,
}

struct History {
    base: Option<Value>,
    steps: Vec<Step>,
    head: Option<Value>,
    tree: Option<Value>,
}

/// Fail when a policy file differs from its reference, unless each commit that changed it carries a `Policy-Change: <path>` trailer.
pub fn check(root: &Path, flags: &[String]) -> Result<(), Failure> {
    let base = repo::base(root, parse(flags)?)?;
    let commits = commits(root, &base)?;
    let mut problems = Vec::new();
    for item in items(root, &base)? {
        problems.extend(judge(&item.path, &history(root, &base, &item, &commits)?));
    }
    verdict(problems)
}

fn parse(flags: &[String]) -> Result<Option<&str>, Failure> {
    match flags {
        [] => Ok(None),
        [flag, base] if flag == "--base" && !base.starts_with("--") => Ok(Some(base)),
        _ => Err(Failure::Usage),
    }
}

fn judge(path: &str, history: &History) -> Vec<String> {
    let (reference, creation) = reference(history);
    if history.tree.as_ref() == reference {
        return Vec::new();
    }
    if history.tree != history.head {
        return vec![format!(
            "{path}: the working tree changes this policy file: commit the change with a `{TRAILER}: {path}` trailer"
        )];
    }
    let changes: Vec<&Step> = history
        .steps
        .iter()
        .filter(|step| Some(step.commit.as_str()) != creation && changed(step))
        .collect();
    if changes.is_empty() {
        return vec![format!(
            "{path}: this policy file differs from its reference, but no commit after the reference changes it"
        )];
    }
    changes
        .iter()
        .filter(|step| !step.approved)
        .map(|step| {
            format!(
                "{path}: commit {} changes this policy file without a `{TRAILER}: {path}` trailer",
                step.commit
            )
        })
        .collect()
}

fn reference(history: &History) -> (Option<&Value>, Option<&str>) {
    if history.base.is_some() {
        return (history.base.as_ref(), None);
    }
    // A file the base lacks takes its version at the commit that created it.
    history
        .steps
        .iter()
        .find(|step| step.value.is_some())
        .map_or((None, None), |step| {
            (step.value.as_ref(), Some(step.commit.as_str()))
        })
}

fn changed(step: &Step) -> bool {
    step.parents.iter().all(|parent| *parent != step.value)
}

fn items(root: &Path, base: &str) -> Result<Vec<Item>, Failure> {
    let range = format!("{base}..HEAD");
    // Ignored files count: clippy reads them too.
    let listed = repo::git(root, &["ls-files", "-z", "--cached", "--others"])?;
    let touched = repo::git(
        root,
        &[
            "log",
            "-z",
            "-m",
            "--no-renames",
            "--name-only",
            "--format=",
            &range,
        ],
    )?;
    let mut clippy = clippy_paths(&listed)?;
    clippy.extend(clippy_paths(&touched)?);
    clippy.sort();
    clippy.dedup();
    let files = FILES
        .iter()
        .map(|&(path, format)| (path.to_owned(), format));
    let manifest = (MANIFEST.to_owned(), Format::Manifest);
    let clippy = clippy.into_iter().map(|path| (path, Format::Toml));
    Ok(files
        .chain([manifest])
        .chain(clippy)
        .map(|(path, format)| Item { path, format })
        .collect())
}

fn clippy_paths(output: &[u8]) -> Result<Vec<String>, Failure> {
    let text = std::str::from_utf8(output)
        .map_err(|error| Failure::Broken(format!("git path listing: {error}")))?;
    Ok(text
        .split(['\0', '\n'])
        .filter(|path| is_clippy(path))
        .map(str::to_owned)
        .collect())
}

fn is_clippy(path: &str) -> bool {
    // Compare in lowercase: a case-insensitive file system (macOS) serves Clippy.toml as clippy.toml.
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    CLIPPY_NAMES.contains(&name.as_str())
}

fn commits(root: &Path, base: &str) -> Result<HashMap<String, Commit>, Failure> {
    let format = format!("--format=%H %P%x1f%(trailers:key={TRAILER},valueonly,separator=%x1f)");
    let output = repo::git(root, &["log", "-z", &format, &format!("{base}..HEAD")])?;
    let text = String::from_utf8(output)
        .map_err(|error| Failure::Broken(format!("git log {base}..HEAD: {error}")))?;
    text.split('\0')
        .filter(|record| !record.trim().is_empty())
        .map(commit)
        .collect()
}

fn commit(record: &str) -> Result<(String, Commit), Failure> {
    let malformed = || Failure::Broken(format!("git log printed a malformed record: {record:?}"));
    let (ids, trailers) = record.split_once('\x1f').ok_or_else(malformed)?;
    let mut ids = ids.split_whitespace().map(str::to_owned);
    let id = ids.next().ok_or_else(malformed)?;
    let trailers = trailers
        .split('\x1f')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect();
    Ok((
        id,
        Commit {
            parents: ids.collect(),
            trailers,
        },
    ))
}

fn history(
    root: &Path,
    base: &str,
    item: &Item,
    commits: &HashMap<String, Commit>,
) -> Result<History, Failure> {
    let mut steps = Vec::new();
    for id in repo::touching(root, base, &item.path)? {
        let commit = commits.get(&id).ok_or_else(|| {
            Failure::Broken(format!("git log {base}..HEAD does not list commit {id}"))
        })?;
        let parents = commit
            .parents
            .iter()
            .map(|parent| version(root, parent, item))
            .collect::<Result<_, _>>()?;
        steps.push(Step {
            approved: commit.trailers.contains(&item.path),
            value: version(root, &id, item)?,
            parents,
            commit: id,
        });
    }
    Ok(History {
        base: version(root, base, item)?,
        steps,
        head: version(root, "HEAD", item)?,
        tree: tree_version(root, item)?,
    })
}

fn version(root: &Path, revision: &str, item: &Item) -> Result<Option<Value>, Failure> {
    let source = format!("{revision}:{}", item.path);
    repo::show(root, revision, &item.path)?
        .map(|bytes| item.format.value(&bytes, &source))
        .transpose()
}

fn tree_version(root: &Path, item: &Item) -> Result<Option<Value>, Failure> {
    let path = root.join(&item.path);
    if !repo::exists(&path)? {
        return Ok(None);
    }
    item.format
        .value(repo::read(&path)?.as_bytes(), &item.path)
        .map(Some)
}

impl Format {
    fn value(self, bytes: &[u8], source: &str) -> Result<Value, Failure> {
        let parsed = match self {
            Self::Json => serde_json::from_slice(bytes).map_err(|error| error.to_string()),
            Self::Toml => toml_table(bytes).and_then(|table| json(&table)),
            Self::Manifest => toml_table(bytes).and_then(|table| json(&picked(&table))),
        };
        parsed.map_err(|error| Failure::Broken(format!("{source}: {error}")))
    }
}

fn toml_table(bytes: &[u8]) -> Result<Table, String> {
    let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    toml::from_str(text).map_err(|error| error.to_string())
}

fn json(table: &Table) -> Result<Value, String> {
    serde_json::to_value(table).map_err(|error| error.to_string())
}

fn picked(manifest: &Table) -> Table {
    MANIFEST_TABLES
        .iter()
        .filter_map(|keys| {
            let (first, rest) = keys.split_first()?;
            let found = rest
                .iter()
                .try_fold(manifest.get(*first)?, |value, key| value.get(key))?;
            Some((keys.join("."), found.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{Format, History, Step, commit, is_clippy, judge, parse};
    use crate::Failure;

    const PATH: &str = "clippy.toml";

    fn step(commit: &str, approved: bool, value: Option<Value>, parents: &[Option<Value>]) -> Step {
        Step {
            commit: commit.to_owned(),
            approved,
            value,
            parents: parents.to_vec(),
        }
    }

    fn lines(limit: u64) -> Value {
        json!({"too-many-lines-threshold": limit})
    }

    fn at_head(base: Option<Value>, steps: Vec<Step>) -> History {
        let head = steps.last().and_then(|step| step.value.clone());
        History {
            base,
            steps,
            tree: head.clone(),
            head,
        }
    }

    #[test]
    fn a_loosening_needs_a_trailer_on_the_commit_that_made_it() {
        let loosened = |approved| {
            at_head(
                Some(lines(60)),
                vec![step("c1", approved, Some(lines(100)), &[Some(lines(60))])],
            )
        };
        assert_eq!(
            judge(PATH, &loosened(false)),
            [
                "clippy.toml: commit c1 changes this policy file without a `Policy-Change: clippy.toml` trailer"
            ]
        );
        assert!(judge(PATH, &loosened(true)).is_empty());
    }

    #[test]
    fn a_file_the_base_lacks_takes_its_creation_commit_as_the_reference() {
        let created = step("c1", false, Some(lines(60)), &[None]);
        let same = at_head(None, vec![created]);
        assert!(judge(PATH, &same).is_empty());
        let created = step("c1", false, Some(lines(60)), &[None]);
        let loosened = step("c2", false, Some(lines(100)), &[Some(lines(60))]);
        let problems = judge(PATH, &at_head(None, vec![created, loosened]));
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("commit c2 changes"), "{problems:?}");
    }

    #[test]
    fn an_uncommitted_change_fails_even_after_approved_commits() {
        let mut history = at_head(
            Some(lines(60)),
            vec![step("c1", true, Some(lines(80)), &[Some(lines(60))])],
        );
        history.tree = Some(lines(100));
        assert_eq!(
            judge(PATH, &history),
            [
                "clippy.toml: the working tree changes this policy file: commit the change with a `Policy-Change: clippy.toml` trailer"
            ]
        );
        history.tree = Some(lines(60));
        assert!(
            judge(PATH, &history).is_empty(),
            "a tree back at the reference passes"
        );
    }

    #[test]
    fn a_merge_that_keeps_one_parent_value_changes_nothing() {
        let approved = step("c1", true, Some(lines(100)), &[Some(lines(60))]);
        let parents = [Some(lines(60)), Some(lines(100))];
        let merge = step("m1", false, Some(lines(100)), &parents);
        assert!(judge(PATH, &at_head(Some(lines(60)), vec![approved, merge])).is_empty());
        let unchanged = step("m2", true, Some(lines(100)), &[Some(lines(100))]);
        let unexplained = at_head(Some(lines(60)), vec![unchanged]);
        assert_eq!(judge(PATH, &unexplained).len(), 1);
    }

    #[test]
    fn values_compare_parsed_so_comments_and_layout_do_not_count() {
        let value = |text: &str| Format::Toml.value(text.as_bytes(), PATH).ok();
        assert_eq!(value("a = 1\nb = 2\n"), value("# why\nb = 2\na   = 1\n"));
        assert_ne!(value("a = 1\n"), value("a = 2\n"));
        let manifest = |text: &str| Format::Manifest.value(text.as_bytes(), "Cargo.toml").ok();
        let lints = "[workspace.lints.clippy]\ntodo = \"forbid\"\n";
        assert_eq!(
            manifest(lints),
            manifest(&format!("[workspace.dependencies]\nserde = \"1\"\n{lints}"))
        );
        assert_ne!(manifest(lints), manifest(&lints.replace("forbid", "deny")));
        let patched = format!("{lints}[patch.crates-io]\n");
        assert_ne!(manifest(lints), manifest(&patched));
        assert_eq!(
            Format::Json.value(b"{\"a\": [1]}", "x").ok(),
            Some(json!({"a": [1]}))
        );
    }

    #[test]
    fn a_log_record_gives_the_commit_its_parents_and_its_trailers() {
        let (id, found) = commit("c3 p1 p2\x1fCargo.toml\x1f deny.toml ").unwrap();
        assert_eq!(id, "c3");
        assert_eq!(found.parents, ["p1", "p2"]);
        assert_eq!(found.trailers, ["Cargo.toml", "deny.toml"]);
        assert!(commit("c4 p1\x1f").unwrap().1.trailers.is_empty());
        assert!(matches!(commit("no separator"), Err(Failure::Broken(_))));
    }

    #[test]
    fn clippy_config_names_compare_without_case_and_flags_parse_once() {
        assert!(is_clippy("crates/io/Clippy.toml"));
        assert!(is_clippy(".clippy.toml"));
        assert!(!is_clippy("clippy.toml.orig"));
        let words = |words: &[&str]| {
            words
                .iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>()
        };
        assert!(matches!(parse(&words(&[])), Ok(None)));
        let main = words(&["--base", "main"]);
        assert!(matches!(parse(&main), Ok(Some("main"))));
        let refused = [
            &["--base"][..],
            &["--base", "--update"],
            &["--base", "a", "--base", "b"],
        ];
        for flags in refused.map(words) {
            assert!(matches!(parse(&flags), Err(Failure::Usage)), "{flags:?}");
        }
    }
}
