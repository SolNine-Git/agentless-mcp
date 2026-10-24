use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;

use crate::{Failure, lint_tables, process, repo, workspace};

// A hang guard, far above the call's run time.
const LINT_TABLE: Duration = Duration::from_secs(10);
const LINT_SECTIONS: [&str; 2] = [
    "Lint checks provided by rustc:",
    "Lint checks loaded by this crate:",
];
const GROUP_SECTIONS: [&str; 2] = [
    "Lint groups provided by rustc:",
    "Lint groups loaded by this crate:",
];
const OPTIONS_SECTION: &str = "Available lint options:";
/// The group of every lint at warn level. An allow of it lowers them all.
pub const WARNINGS: &str = "warnings";
// --force-warn refuses `warnings` (E0602), and a stable rustc calls these two feature-gated lints unknown.
const NOT_FORCED: [&str; 3] = [
    WARNINGS,
    "default_overrides_default_fields",
    "test_unstable_lint",
];
const CFG_LINT: &str = "unexpected_cfgs";
const MANIFEST: &str = "Cargo.toml";

/// A lint level, as `-W help` and the lint tables of a manifest spell it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Allow,
    Warn,
    Deny,
    Forbid,
}

/// Each lint that clippy-driver knows, with its default level, and each lint group with its members.
pub struct Table {
    pub levels: BTreeMap<String, Level>,
    pub groups: BTreeMap<String, Vec<String>>,
}

/// One entry of [workspace.lints]: the lint or group it names with its tool prefix, and what it sets.
#[derive(Debug, PartialEq, Eq)]
pub struct Configured {
    pub name: String,
    pub level: Level,
    pub priority: i64,
    pub check_cfg: bool,
}

#[derive(Deserialize)]
struct Manifest {
    workspace: Workspace,
}

#[derive(Deserialize)]
struct Workspace {
    lints: BTreeMap<String, BTreeMap<String, Setting>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Setting {
    Level(String),
    Detailed(Detailed),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Detailed {
    level: String,
    #[serde(default)]
    priority: i64,
    #[serde(rename = "check-cfg")]
    check_cfg: Option<Vec<String>>,
}

impl Level {
    fn parse(word: &str) -> Result<Self, String> {
        match word {
            "allow" => Ok(Self::Allow),
            "warn" => Ok(Self::Warn),
            "deny" => Ok(Self::Deny),
            "forbid" => Ok(Self::Forbid),
            other => Err(format!("the lint level `{other}` is unknown")),
        }
    }
}

/// Read the lint table that `clippy-driver -W help` prints for `edition`, under its deadline.
pub fn table(root: &Path, edition: &str) -> Result<Table, Failure> {
    let output = process::stdout(&mut help(root, edition), LINT_TABLE)?;
    String::from_utf8(output)
        .map_err(|error| error.to_string())
        .and_then(|text| parse_table(&text))
        .map_err(|error| Failure::Broken(format!("clippy-driver -W help: {error}")))
}

/// Read the [workspace.lints] entries of the root manifest.
pub fn configured(root: &Path) -> Result<Vec<Configured>, Failure> {
    let path = root.join(MANIFEST);
    parse_configured(&repo::read(&path)?).map_err(|error| repo::broken(&path, &error))
}

/// Read the [workspace.package] edition of the root manifest, which every target builds with.
pub fn edition(root: &Path) -> Result<String, Failure> {
    let manifest: toml::Table = repo::toml(&root.join(MANIFEST))?;
    workspace::package_key(&manifest, "edition")
}

// A lint's default level can depend on the edition: edition 2024 warns on unsafe_op_in_unsafe_fn.
fn help(root: &Path, edition: &str) -> Command {
    let mut command = Command::new("clippy-driver");
    command
        .arg(format!("--edition={edition}"))
        .args(["-W", "help"])
        .current_dir(root);
    command
}

/// Return each lint's level in the normal clippy run: its default, then the workspace entries by priority.
pub fn levels(table: &Table, configured: &[Configured]) -> Result<BTreeMap<String, Level>, String> {
    let mut by_priority: BTreeMap<i64, Vec<&Configured>> = BTreeMap::new();
    for entry in configured {
        by_priority.entry(entry.priority).or_default().push(entry);
    }
    let mut levels = table.levels.clone();
    for entries in by_priority.values() {
        levels.extend(assignments(table, entries)?);
    }
    Ok(levels)
}

/// Return each lint that the normal run warns about or denies: the lints that a source attribute could lower.
pub fn force_warned(levels: &BTreeMap<String, Level>) -> Vec<String> {
    // rustc and clippy-driver refuse an attribute that lowers a forbidden lint (E0453), so forbid needs no force-warn.
    levels
        .iter()
        .filter(|(name, level)| {
            matches!(level, Level::Warn | Level::Deny) && !NOT_FORCED.contains(&name.as_str())
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// Return a problem unless the workspace forbids each cfg that rustc and the package features do not declare.
pub fn cfg_problems(levels: &BTreeMap<String, Level>, configured: &[Configured]) -> Vec<String> {
    let mut problems = Vec::new();
    if levels.get(CFG_LINT) != Some(&Level::Forbid) {
        problems.push(format!(
            "{MANIFEST}: [workspace.lints.rust] does not set {CFG_LINT} = \"forbid\": a cfg that rustc does not know can then hide code from clippy. Set that level"
        ));
    }
    if configured
        .iter()
        .any(|entry| entry.name == CFG_LINT && entry.check_cfg)
    {
        problems.push(format!(
            "{MANIFEST}: {CFG_LINT} has a check-cfg list, and each cfg in it can hide code from clippy. Remove the list, and use a cfg that rustc knows"
        ));
    }
    problems
}

fn parse_table(text: &str) -> Result<Table, String> {
    let sections = sections(text)?;
    let mut levels = BTreeMap::new();
    for title in LINT_SECTIONS {
        for (name, rest) in rows(&sections, title)? {
            let word = rest.split_whitespace().next().unwrap_or_default();
            levels.insert(name, Level::parse(word)?);
        }
    }
    let mut groups = BTreeMap::new();
    for title in GROUP_SECTIONS {
        for (name, rest) in rows(&sections, title)? {
            groups.insert(name, rest.split(',').map(lint_name).collect::<Vec<_>>());
        }
    }
    groups.remove(WARNINGS);
    match groups
        .values()
        .flatten()
        .find(|member| !levels.contains_key(*member))
    {
        Some(member) => Err(format!("the group member {member} is in no lint table")),
        None => Ok(Table { levels, groups }),
    }
}

// Each row is indented, so an unindented line is a title, and an unknown one means the format changed.
fn sections(text: &str) -> Result<BTreeMap<&str, Vec<&str>>, String> {
    let titles: Vec<&str> = [OPTIONS_SECTION]
        .into_iter()
        .chain(LINT_SECTIONS)
        .chain(GROUP_SECTIONS)
        .collect();
    let mut sections: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut current = None;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        if line.starts_with(char::is_whitespace) {
            let title = current.ok_or_else(|| format!("the row `{line}` has no section"))?;
            sections.entry(title).or_default().push(line);
        } else if titles.contains(&line) {
            current = Some(line);
        } else {
            return Err(format!("the line `{line}` is not a known section title"));
        }
    }
    Ok(sections)
}

fn rows(
    sections: &BTreeMap<&str, Vec<&str>>,
    title: &str,
) -> Result<Vec<(String, String)>, String> {
    let lines = sections
        .get(title)
        .ok_or_else(|| format!("the section `{title}` is missing"))?;
    let rows: Vec<(String, String)> = lines
        .iter()
        .filter_map(|line| line.trim().split_once(char::is_whitespace))
        .filter(|(name, _)| *name != "name" && !name.starts_with("----"))
        .map(|(name, rest)| (lint_name(name), rest.trim().to_owned()))
        .collect();
    if rows.is_empty() {
        Err(format!("the section `{title}` has no rows"))
    } else {
        Ok(rows)
    }
}

fn lint_name(name: &str) -> String {
    name.trim().replace('-', "_")
}

fn parse_configured(text: &str) -> Result<Vec<Configured>, String> {
    let manifest: Manifest = toml::from_str(text).map_err(|error| error.to_string())?;
    let mut configured = Vec::new();
    for (tool, table) in manifest.workspace.lints {
        let prefix = lint_tables::lint_prefix(&tool).ok_or_else(|| {
            format!(
                "[workspace.lints.{tool}]: the ground-truth check reads only the rust and clippy tables. Teach xtask/src/lint_tables.rs the tool in review"
            )
        })?;
        for (name, setting) in table {
            configured.push(setting.configured(format!("{prefix}{}", lint_name(&name)))?);
        }
    }
    Ok(configured)
}

impl Setting {
    fn configured(self, name: String) -> Result<Configured, String> {
        let (level, priority, check_cfg) = match self {
            Self::Level(level) => (level, 0, false),
            Self::Detailed(detailed) => (
                detailed.level,
                detailed.priority,
                detailed.check_cfg.is_some(),
            ),
        };
        Ok(Configured {
            name,
            level: Level::parse(&level)?,
            priority,
            check_cfg,
        })
    }
}

// cargo orders the entries of one priority in no documented way, so two of them must not disagree on a lint.
fn assignments(table: &Table, entries: &[&Configured]) -> Result<BTreeMap<String, Level>, String> {
    let mut assigned = BTreeMap::new();
    for entry in entries {
        for lint in members(table, &entry.name)? {
            let before = assigned.insert(lint.clone(), entry.level);
            if before.is_some_and(|level| level != entry.level) {
                return Err(format!(
                    "[workspace.lints] sets {lint} to two levels at priority {}. Give the group a lower priority than the lint",
                    entry.priority
                ));
            }
        }
    }
    Ok(assigned)
}

fn members(table: &Table, name: &str) -> Result<Vec<String>, String> {
    if let Some(members) = table.groups.get(name) {
        Ok(members.clone())
    } else if table.levels.contains_key(name) {
        Ok(vec![name.to_owned()])
    } else {
        Err(format!(
            "[workspace.lints] names {name}, which clippy-driver -W help does not list. Use a lint or group name that it lists"
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        Configured, Level, Table, cfg_problems, force_warned, help, levels, parse_configured,
        parse_table,
    };

    const HELP: &str = "
Available lint options:
    -W <foo>           Warn about <foo>

Lint checks provided by rustc:

                         name  default  meaning
                         ----  -------  -------
                    dead-code  warn     detects unused items
             unused-variables  warn     detects unused variables
    elided-lifetimes-in-paths  allow    hidden lifetime parameters
          arithmetic-overflow  deny     detects overflow
              unexpected-cfgs  warn     detects unknown cfgs
                     warnings  warn     mentions every warning
           test-unstable-lint  deny     a feature-gated lint

Lint groups provided by rustc:

                         name  sub-lints
                         ----  ---------
                     warnings  all lints that are set to issue warnings
                       unused  unused-variables, dead-code

Lint checks loaded by this crate:

                         name  default  meaning
                         ----  -------  -------
              clippy::ptr-arg  warn     writing &Vec instead of &[_]
   clippy::must-use-candidate  allow    function could have a #[must_use] attribute
            clippy::dbg-macro  allow    dbg! is a debugging tool

Lint groups loaded by this crate:

                         name  sub-lints
                         ----  ---------
                  clippy::all  clippy::ptr-arg
             clippy::pedantic  clippy::must-use-candidate
";

    fn table() -> Table {
        parse_table(HELP).unwrap()
    }

    fn entry(name: &str, level: Level, priority: i64) -> Configured {
        Configured {
            name: name.to_owned(),
            level,
            priority,
            check_cfg: false,
        }
    }

    #[test]
    fn the_lint_table_reads_each_level_and_group_with_underscores() {
        let table = table();
        assert_eq!(table.levels["dead_code"], Level::Warn);
        assert_eq!(table.levels["arithmetic_overflow"], Level::Deny);
        assert_eq!(table.levels["clippy::must_use_candidate"], Level::Allow);
        assert_eq!(table.groups["unused"], ["unused_variables", "dead_code"]);
        assert_eq!(
            table.groups["clippy::pedantic"],
            ["clippy::must_use_candidate"]
        );
        assert!(!table.groups.contains_key("warnings"));
    }

    #[test]
    fn a_lint_table_with_a_missing_or_empty_section_an_unknown_level_or_member_is_refused() {
        let clippy_rows = HELP
            .lines()
            .filter(|line| line.trim_start().starts_with("clippy::") && !line.contains(','))
            .filter(|line| !line.contains("clippy::all") && !line.contains("clippy::pedantic"));
        let empty = clippy_rows.fold(HELP.to_owned(), |text, row| {
            text.replace(&format!("{row}\n"), "")
        });
        for (text, error) in [
            (
                HELP.split("Lint groups loaded by this crate:")
                    .next()
                    .unwrap()
                    .to_owned(),
                "the section `Lint groups loaded by this crate:` is missing",
            ),
            (
                HELP.replace("Lint groups loaded by this crate:", "Lint groups:"),
                "the line `Lint groups:` is not a known section title",
            ),
            (
                empty,
                "the section `Lint checks loaded by this crate:` has no rows",
            ),
            (
                HELP.replace("dead-code  warn", "dead-code  sometimes"),
                "the lint level `sometimes` is unknown",
            ),
            (
                HELP.replace(
                    "unused-variables, dead-code",
                    "unused-variables, unused-mut",
                ),
                "the group member unused_mut is in no lint table",
            ),
        ] {
            assert_eq!(parse_table(&text).err().unwrap(), error);
        }
    }

    #[test]
    fn workspace_entries_apply_by_priority_and_a_group_sets_its_members() {
        let configured = [
            entry("clippy::pedantic", Level::Warn, -1),
            entry("clippy::dbg_macro", Level::Forbid, 0),
            entry("dead_code", Level::Allow, 0),
        ];
        let levels = levels(&table(), &configured).unwrap();
        assert_eq!(levels["clippy::must_use_candidate"], Level::Warn);
        assert_eq!(levels["clippy::dbg_macro"], Level::Forbid);
        assert_eq!(levels["dead_code"], Level::Allow);
        let later = [
            entry("clippy::pedantic", Level::Warn, -1),
            entry("clippy::must_use_candidate", Level::Allow, 0),
        ];
        let levels = super::levels(&table(), &later).unwrap();
        assert_eq!(levels["clippy::must_use_candidate"], Level::Allow);
    }

    #[test]
    fn the_force_warned_set_is_each_warn_and_deny_lint_but_warnings_and_feature_gated_lints() {
        let configured = [
            entry("clippy::pedantic", Level::Warn, -1),
            entry("clippy::dbg_macro", Level::Forbid, 0),
            entry("unexpected_cfgs", Level::Forbid, 0),
        ];
        let levels = levels(&table(), &configured).unwrap();
        assert_eq!(
            force_warned(&levels),
            [
                "arithmetic_overflow",
                "clippy::must_use_candidate",
                "clippy::ptr_arg",
                "dead_code",
                "unused_variables",
            ]
        );
    }

    #[test]
    fn two_entries_of_one_priority_that_disagree_or_an_unknown_name_are_refused() {
        let disagree = [
            entry("unused", Level::Warn, 0),
            entry("dead_code", Level::Allow, 0),
        ];
        assert_eq!(
            levels(&table(), &disagree).err().unwrap(),
            "[workspace.lints] sets dead_code to two levels at priority 0. Give the group a lower priority than the lint"
        );
        let unknown = [entry("clippy::no_such_lint", Level::Warn, 0)];
        assert!(
            levels(&table(), &unknown)
                .err()
                .unwrap()
                .starts_with("[workspace.lints] names clippy::no_such_lint")
        );
    }

    #[test]
    fn the_cfg_lint_must_be_forbidden_and_have_no_check_cfg_list() {
        let forbidden = [entry("unexpected_cfgs", Level::Forbid, 0)];
        let levels_forbidden = levels(&table(), &forbidden).unwrap();
        assert!(cfg_problems(&levels_forbidden, &forbidden).is_empty());
        let default = levels(&table(), &[]).unwrap();
        assert_eq!(cfg_problems(&default, &[]).len(), 1);
        let listed = [Configured {
            check_cfg: true,
            ..entry("unexpected_cfgs", Level::Forbid, 0)
        }];
        let problems = cfg_problems(&levels(&table(), &listed).unwrap(), &listed);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("check-cfg"), "{problems:?}");
    }

    #[test]
    fn the_workspace_lints_parse_reads_levels_priorities_and_check_cfg() {
        let text = "[workspace]\nmembers = []\n[workspace.lints.rust]\n\
                    unexpected_cfgs = { level = \"forbid\", check-cfg = [\"cfg(x)\"] }\n\
                    [workspace.lints.clippy]\npedantic = { level = \"warn\", priority = -1 }\ntodo = \"forbid\"\n";
        assert_eq!(
            parse_configured(text).unwrap(),
            [
                entry("clippy::pedantic", Level::Warn, -1),
                entry("clippy::todo", Level::Forbid, 0),
                Configured {
                    check_cfg: true,
                    ..entry("unexpected_cfgs", Level::Forbid, 0)
                },
            ]
        );
    }

    #[test]
    fn the_lint_table_is_read_for_the_workspace_edition() {
        let command = help(Path::new("/r"), "2024");
        let args: Vec<&str> = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        assert_eq!(args, ["--edition=2024", "-W", "help"]);
    }

    #[test]
    fn an_unknown_key_level_or_tool_in_the_workspace_lints_is_refused() {
        for text in [
            "[workspace.lints.rust]\ndead_code = { level = \"warn\", weight = 1 }\n",
            "[workspace.lints.rust]\ndead_code = \"sometimes\"\n",
            "[workspace.lints.rustdoc]\nbroken_intra_doc_links = \"deny\"\n",
            "[workspace]\nmembers = []\n",
        ] {
            assert!(parse_configured(text).is_err(), "{text}");
        }
    }
}
