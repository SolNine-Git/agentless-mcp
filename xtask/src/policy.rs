use std::collections::HashMap;
use std::path::Path;

use toml::Table;

use crate::{Failure, repo, tool_config, verdict};

const TRAILER: &str = "Policy-Change";
const FILES: [(&str, Format); 7] = [
    (".agentless-mcp.json", Format::Json),
    ("deny.toml", Format::Toml),
    ("rustfmt.toml", Format::Toml),
    ("rust-toolchain.toml", Format::Toml),
    (".config/nextest.toml", Format::Toml),
    ("crates/layers.toml", Format::Toml),
    ("crates/dependencies.toml", Format::Toml),
];
const MANIFEST: &str = "Cargo.toml";

#[derive(Clone, Copy)]
enum Format {
    Json,
    Toml,
    Manifest,
}

// Each file compares in its own format: a TOML float keeps NaN and infinity, and a NaN equals nothing.
#[derive(Clone, Debug, PartialEq)]
enum Value {
    Json(serde_json::Value),
    Toml(Table),
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

enum Reference<'a> {
    Base(Option<&'a Value>),
    Created { value: &'a Value, commit: &'a str },
    SeveralCreations,
}

/// Fail when a policy file at HEAD or in the working tree differs from its reference, unless each commit that changed it, merges included, carries a `Policy-Change: <path>` trailer and one of them made HEAD's value.
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
    let reference = reference(history);
    let mut problems = head_problems(path, history, &reference);
    // head_problems judges HEAD, so the tree only has to match HEAD or undo HEAD's change.
    if history.tree != history.head && !reference.holds(history.tree.as_ref()) {
        problems.push(format!(
            "{path}: the working tree changes this policy file: commit the change with a `{TRAILER}: {path}` trailer"
        ));
    }
    problems
}

fn head_problems(path: &str, history: &History, reference: &Reference) -> Vec<String> {
    let head = history.head.as_ref();
    if reference.holds(head) {
        return Vec::new();
    }
    let exempt = reference.exempt();
    let changes: Vec<&Step> = history
        .steps
        .iter()
        .filter(|step| Some(step.commit.as_str()) != exempt && changed(step))
        .collect();
    let mut problems: Vec<String> = changes
        .iter()
        .filter(|step| !step.approved)
        .map(|step| {
            format!(
                "{path}: commit {} changes this policy file without a `{TRAILER}: {path}` trailer. Reword that commit with the trailer, or revert its change",
                step.commit
            )
        })
        .collect();
    // A merge that keeps a parent's value changes nothing, so a parent from outside the range can
    // bring a value that no commit in the range made.
    if !changes.iter().any(|step| step.value.as_ref() == head) {
        problems.push(format!(
            "{path}: HEAD holds a value that no commit after the reference made: a merge brought it from outside the range. Redo the merge so that it keeps the value from inside the range, then change the value in a commit with a `{TRAILER}: {path}` trailer"
        ));
    }
    problems
}

fn reference(history: &History) -> Reference<'_> {
    if history.base.is_some() {
        return Reference::Base(history.base.as_ref());
    }
    // A file the base lacks takes its version at the one commit that created it. With several, a
    // merge's parent order would pick one, so none is the reference.
    let mut creations = history.steps.iter().filter_map(creation);
    match (creations.next(), creations.next()) {
        (None, _) => Reference::Base(None),
        (Some((value, commit)), None) => Reference::Created { value, commit },
        (Some(_), Some(_)) => Reference::SeveralCreations,
    }
}

// A creation adds the file where every parent lacks it, a root commit included.
fn creation(step: &Step) -> Option<(&Value, &str)> {
    let value = step.value.as_ref()?;
    step.parents
        .iter()
        .all(Option::is_none)
        .then_some((value, step.commit.as_str()))
}

impl Reference<'_> {
    fn holds(&self, value: Option<&Value>) -> bool {
        match self {
            Self::Base(reference) => *reference == value,
            Self::Created {
                value: reference, ..
            } => Some(*reference) == value,
            Self::SeveralCreations => false,
        }
    }

    fn exempt(&self) -> Option<&str> {
        match self {
            Self::Created { commit, .. } => Some(commit),
            Self::Base(_) | Self::SeveralCreations => None,
        }
    }
}

fn changed(step: &Step) -> bool {
    step.parents.iter().all(|parent| *parent != step.value)
}

fn items(root: &Path, base: &str) -> Result<Vec<Item>, Failure> {
    let range = format!("{base}..HEAD");
    // The base and HEAD trees name a file that the index dropped. The tool-config gate owns
    // untracked and ignored files.
    let listings = [
        repo::git(root, &["ls-files", "-z", "--cached"])?,
        // -m lists the paths a merge changes, and --no-renames keeps both sides of a rename.
        repo::git(
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
        )?,
        repo::git(root, &["ls-tree", "-r", "-z", "--name-only", base])?,
        repo::git(root, &["ls-tree", "-r", "-z", "--name-only", "HEAD"])?,
    ];
    let mut clippy = Vec::new();
    for listing in &listings {
        clippy.extend(clippy_paths(listing)?);
    }
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
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    tool_config::CLIPPY_NAMES.contains(&name.as_str())
}

fn commits(root: &Path, base: &str) -> Result<HashMap<String, Commit>, Failure> {
    let output = repo::git(
        root,
        &["log", "-z", "--format=%H %P", &format!("{base}..HEAD")],
    )?;
    let text = String::from_utf8(output)
        .map_err(|error| Failure::Broken(format!("git log {base}..HEAD: {error}")))?;
    let mut commits = HashMap::new();
    for record in text.split('\0').filter(|record| !record.trim().is_empty()) {
        let (id, parents) = ids(record)?;
        // git's trailer parser follows core.commentChar and trailer aliases. The raw object does not.
        let object = repo::git(root, &["cat-file", "commit", &id])?;
        let trailers = trailers(&object);
        commits.insert(id, Commit { parents, trailers });
    }
    Ok(commits)
}

fn ids(record: &str) -> Result<(String, Vec<String>), Failure> {
    let mut ids = record.split_whitespace().map(str::to_owned);
    let id = ids.next().ok_or_else(|| {
        Failure::Broken(format!("git log printed a malformed record: {record:?}"))
    })?;
    Ok((id, ids.collect()))
}

// The trailer block is the message's last paragraph, never its title. A line approves a path only
// when it is exactly `Policy-Change: <path>`, so a malformed line approves nothing.
fn trailers(object: &[u8]) -> Vec<String> {
    let message = object
        .windows(2)
        .position(|pair| pair == b"\n\n")
        .map_or(&[][..], |end| &object[end + 2..]);
    let lines: Vec<&[u8]> = message.split(|&byte| byte == b'\n').collect();
    let paragraphs: Vec<&[&[u8]]> = lines
        .split(|line| line.iter().all(u8::is_ascii_whitespace))
        .filter(|paragraph| !paragraph.is_empty())
        .collect();
    match paragraphs.as_slice() {
        [_title, .., last] => last.iter().copied().filter_map(approval).collect(),
        _ => Vec::new(),
    }
}

fn approval(line: &[u8]) -> Option<String> {
    let path = std::str::from_utf8(line)
        .ok()?
        .strip_prefix(TRAILER)?
        .strip_prefix(": ")?;
    let clean = !path.is_empty() && !path.chars().any(char::is_control);
    clean.then(|| path.to_owned())
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
            Self::Json => serde_json::from_slice(bytes)
                .map(Value::Json)
                .map_err(|error| error.to_string()),
            Self::Toml => toml_table(bytes).map(Value::Toml),
            Self::Manifest => toml_table(bytes).map(|table| Value::Toml(policy_view(table))),
        };
        parsed.map_err(|error| Failure::Broken(format!("{source}: {error}")))
    }
}

fn toml_table(bytes: &[u8]) -> Result<Table, String> {
    let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    toml::from_str(text).map_err(|error| error.to_string())
}

// Ordinary work changes the dependency list and the version. Every other key is policy, so a key
// that cargo or a gate tool starts to read needs a trailer from the first change.
fn policy_view(mut manifest: Table) -> Table {
    if let Some(toml::Value::Table(workspace)) = manifest.get_mut("workspace") {
        workspace.remove("dependencies");
        if let Some(toml::Value::Table(package)) = workspace.get_mut("package") {
            package.remove("version");
        }
    }
    manifest
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Format, History, Step, Value, commits, ids, is_clippy, judge, parse, trailers};
    use crate::Failure;
    use crate::repo::fixture::{git, scratch};

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
        let text = format!("too-many-lines-threshold = {limit}\n");
        Format::Toml.value(text.as_bytes(), PATH).unwrap()
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
                "clippy.toml: commit c1 changes this policy file without a `Policy-Change: clippy.toml` trailer. Reword that commit with the trailer, or revert its change"
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
    fn with_several_creation_commits_none_is_exempt_whatever_the_parent_order() {
        let sibling = || step("s0", false, Some(lines(1000)), &[None]);
        let created = || step("c0", false, Some(lines(60)), &[None]);
        let tightened = || step("t", true, Some(lines(59)), &[Some(lines(60))]);
        let merge = |parents: &[Option<Value>]| step("m", false, Some(lines(1000)), parents);
        let sibling_first = vec![
            sibling(),
            created(),
            tightened(),
            merge(&[Some(lines(1000)), Some(lines(59))]),
        ];
        let port_first = vec![
            created(),
            tightened(),
            sibling(),
            merge(&[Some(lines(59)), Some(lines(1000))]),
        ];
        for steps in [sibling_first, port_first] {
            let problems = judge(PATH, &at_head(None, steps));
            assert_eq!(problems.len(), 2, "{problems:?}");
            for commit in ["s0", "c0"] {
                let named = format!("commit {commit} changes");
                assert!(
                    problems.iter().any(|problem| problem.contains(&named)),
                    "{problems:?}"
                );
            }
        }
    }

    #[test]
    fn a_file_deleted_and_created_again_needs_a_trailer_on_each_change() {
        let history = |approved| {
            at_head(
                None,
                vec![
                    step("c0", approved, Some(lines(60)), &[None]),
                    step("d1", approved, None, &[Some(lines(60))]),
                    step("c1", approved, Some(lines(1000)), &[None]),
                    step("d2", approved, None, &[Some(lines(1000))]),
                ],
            )
        };
        assert_eq!(judge(PATH, &history(false)).len(), 4);
        assert!(judge(PATH, &history(true)).is_empty());
    }

    #[test]
    fn trailers_come_from_the_raw_message_whatever_the_config_or_a_replace_ref_says() {
        let dir = scratch("policy-trailers");
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "core.commentChar", ";"]);
        git(&dir, &["config", "trailer.pc.key", "Policy-Change"]);
        git(&dir, &["commit", "-q", "--allow-empty", "-m", "base"]);
        let base = git(&dir, &["rev-parse", "HEAD"]);
        let messages = [
            "comment\n\nPolicy-Change: clippy.toml\n\n; reviewed by nobody\n",
            "alias\n\npc: clippy.toml\n",
            "replaced\n",
            "approved\n\nPolicy-Change: clippy.toml\nCo-Authored-By: t <t@invalid>\n",
        ];
        let mut ids = Vec::new();
        for message in messages {
            let words = [
                "commit",
                "-q",
                "--allow-empty",
                "--cleanup=verbatim",
                "-m",
                message,
            ];
            git(&dir, &words);
            ids.push(git(&dir, &["rev-parse", "HEAD"]));
        }
        let tree = git(&dir, &["rev-parse", &format!("{}^{{tree}}", ids[2])]);
        let parent = git(&dir, &["rev-parse", &format!("{}^", ids[2])]);
        let approved_copy = ["-m", "replaced", "-m", "Policy-Change: clippy.toml"];
        let copy = git(
            &dir,
            &[&["commit-tree", &tree, "-p", &parent][..], &approved_copy].concat(),
        );
        git(&dir, &["replace", &ids[2], &copy]);
        let found = commits(&dir, &base);
        std::fs::remove_dir_all(&dir).unwrap();
        let found = found.unwrap();
        let approved: Vec<bool> = ids
            .iter()
            .map(|id| !found[id].trailers.is_empty())
            .collect();
        assert_eq!(approved, [false, false, false, true]);
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
    fn a_merge_that_brings_a_value_from_outside_the_range_fails() {
        let stale = [Some(lines(59)), Some(lines(1000))];
        let tighten = || step("c1", true, Some(lines(59)), &[Some(lines(60))]);
        let merge = step("m1", false, Some(lines(1000)), &stale);
        let problems = judge(PATH, &at_head(Some(lines(60)), vec![tighten(), merge]));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].starts_with(
                "clippy.toml: HEAD holds a value that no commit after the reference made"
            ),
            "{problems:?}"
        );
        let kept = step("m1", false, Some(lines(59)), &stale);
        assert!(judge(PATH, &at_head(Some(lines(60)), vec![tighten(), kept])).is_empty());
    }

    #[test]
    fn head_is_judged_when_the_working_tree_is_back_at_the_reference() {
        let loosened = step("c1", false, Some(lines(100)), &[Some(lines(60))]);
        let mut history = at_head(Some(lines(60)), vec![loosened]);
        history.tree = Some(lines(60));
        assert_eq!(
            judge(PATH, &history),
            [
                "clippy.toml: commit c1 changes this policy file without a `Policy-Change: clippy.toml` trailer. Reword that commit with the trailer, or revert its change"
            ]
        );
    }

    #[test]
    fn a_policy_file_deleted_in_the_working_tree_fails() {
        let history = History {
            base: Some(lines(60)),
            steps: Vec::new(),
            head: Some(lines(60)),
            tree: None,
        };
        assert_eq!(
            judge(PATH, &history),
            [
                "clippy.toml: the working tree changes this policy file: commit the change with a `Policy-Change: clippy.toml` trailer"
            ]
        );
    }

    #[test]
    fn toml_values_compare_as_toml_so_a_nan_fails_closed() {
        let value = |text: &str| Format::Toml.value(text.as_bytes(), PATH).ok();
        assert_ne!(value("x = nan\n"), value("x = inf\n"));
        assert_ne!(value("x = inf\n"), value("x = -inf\n"));
        assert_ne!(value("x = nan\n"), value("x = nan\n"));
    }

    #[test]
    fn the_root_manifest_is_policy_except_its_dependencies_and_version() {
        let manifest = |text: &str| Format::Manifest.value(text.as_bytes(), "Cargo.toml").ok();
        let root = "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.package]\nversion = \"1.0.0\"\nrust-version = \"1.94\"\n\n[workspace.dependencies]\nserde = \"1\"\n";
        let ordinary = root
            .replace("1.0.0", "1.0.1")
            .replace("serde = \"1\"", "serde = \"1.0.200\"\nitoa = \"1\"");
        assert_eq!(manifest(root), manifest(&ordinary));
        let members = "members = [\"crates/*\"]";
        for policy in [
            root.replace(
                members,
                "members = [\"crates/*\"]\nexclude = [\"crates/kernel\"]",
            ),
            root.replace(members, "members = [\"crates/a\"]"),
            root.replace(
                members,
                "members = [\"crates/*\"]\ndefault-members = [\"xtask\"]",
            ),
            root.replace("1.94", "1.0"),
            format!("{root}\n[workspace.metadata.cargo-machete]\nignored = [\"serde\"]\n"),
            format!("{root}\n[profile.dev]\ndebug-assertions = false\n"),
            format!("{root}\n[patch.crates-io]\nserde = {{ path = \"vendor/serde\" }}\n"),
            format!("{root}\n[replace]\n\"serde:1.0.0\" = {{ path = \"vendor/serde\" }}\n"),
        ] {
            assert_ne!(manifest(root), manifest(&policy), "{policy}");
        }
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
            Some(Value::Json(json!({"a": [1]})))
        );
    }

    #[test]
    fn a_log_record_gives_the_commit_and_its_parents() {
        let (id, parents) = ids("c3 p1 p2").unwrap();
        assert_eq!(id, "c3");
        assert_eq!(parents, ["p1", "p2"]);
        assert!(ids("c4 ").unwrap().1.is_empty());
    }

    #[test]
    fn only_an_exact_trailer_line_in_the_last_paragraph_approves() {
        let header = "tree t\nparent p\nauthor a\ncommitter c\ngpgsig -----BEGIN-----\n \n s\n -----END-----\n";
        let found = |message: &str| trailers(format!("{header}\n{message}").as_bytes());
        let block =
            "Policy-Change: Cargo.toml\nPolicy-Change: deny.toml\nCo-Authored-By: t <t@invalid>";
        assert_eq!(
            found(&format!("loosen\n\n{block}\n\n")),
            ["Cargo.toml", "deny.toml"]
        );
        for refused in [
            "loosen\n\nPolicy-Change: clippy.toml\n\n; reviewed by nobody\n",
            "loosen\n\npc: clippy.toml\n",
            "loosen\n\nPolicy-Change: deny.toml\u{1f}clippy.toml\n",
            "loosen\n\nPolicy-Change: clippy.toml\r\n",
            "loosen\n\npolicy-change: clippy.toml\n",
            "loosen\n\nPolicy-Change:clippy.toml\n",
            "loosen\n\nPolicy-Change: \n",
            "Policy-Change: clippy.toml\n",
        ] {
            assert!(found(refused).is_empty(), "{refused:?}");
        }
        assert!(trailers(header.as_bytes()).is_empty());
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
