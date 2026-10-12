//! The language files of crates/syntax/languages/, and the Rust tables made from them.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde::Deserialize;

use super::literal::literal;
use crate::{Failure, repo};

/// The directory of the language files, one per language.
pub const LANGUAGES: &str = "crates/syntax/languages";
/// The directory that the generated tables go to.
pub const OUTPUT: &str = "crates/syntax/src/generated";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    name: String,
    tier: u8,
    extensions: Vec<String>,
    probe: String,
    probe_root: String,
    comments: Option<Vec<String>>,
    sloc: Option<SlocRule>,
    statement_parents: Option<Vec<String>>,
    units: Option<UnitTable>,
    decisions: Option<DecisionTable>,
    clones: Option<CloneTable>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnitTable {
    units: Vec<String>,
    scopes: Vec<String>,
    bound: Vec<String>,
    descend: Option<Vec<String>>,
    name: NameRule,
    binding: Option<BindingRule>,
    owner: Option<OwnerRule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionTable {
    points: Vec<String>,
    conditional: BTreeMap<String, Cond>,
    field_operators: BTreeMap<String, Vec<String>>,
    token_operators: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CloneTable {
    functions: Vec<String>,
    blocks: Vec<String>,
    skip: Option<SkipRule>,
    identifiers: Vec<String>,
    indent_blocks: Vec<String>,
    literals: BTreeMap<LiteralClass, Vec<String>>,
}

// Each enum names the Rust variant that its snake_case spelling in a language file selects.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SlocRule {
    Leaves,
    PythonTokens,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum NameRule {
    Field,
    CDeclarator,
    CppDeclarator,
    RustImpl,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BindingRule {
    Javascript,
    Typescript,
    Lua,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OwnerRule {
    GoReceiver,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SkipRule {
    TypeChecking,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Cond {
    HasNamedChild,
    NotDefaultCase,
    CountedRustArm,
    HasCondition,
    CountedRubyArm,
    CountedCaseItem,
}

#[derive(Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
enum LiteralClass {
    #[serde(rename = "STR")]
    Str,
    #[serde(rename = "NUM")]
    Num,
    #[serde(rename = "INT")]
    Int,
    #[serde(rename = "FLOAT")]
    Float,
    #[serde(rename = "BOOL")]
    Bool,
    #[serde(rename = "NONE")]
    Null,
}

/// Reads every language file, in name order, or gives each problem.
pub fn read(root: &Path) -> Result<Vec<(String, String)>, Failure> {
    let dir = root.join(LANGUAGES);
    let entries = std::fs::read_dir(&dir).map_err(|error| repo::broken(&dir, &error))?;
    let mut files = Vec::new();
    for entry in entries {
        let path = entry.map_err(|error| repo::broken(&dir, &error))?.path();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        files.push((name.unwrap_or_default(), repo::read(&path)?));
    }
    files.sort();
    Ok(files)
}

/// Returns the generated module, or each problem that the language files hold.
pub fn module(files: &[(String, String)]) -> Result<String, Vec<String>> {
    let mut parsed = Vec::new();
    let mut problems = Vec::new();
    for (name, text) in files {
        match parse(name, text) {
            Ok(file) => parsed.push(file),
            Err(problem) => problems.push(format!("{LANGUAGES}/{name}: {problem}")),
        }
    }
    if problems.is_empty() {
        Ok(render(&parsed))
    } else {
        Err(problems)
    }
}

fn parse(name: &str, text: &str) -> Result<File, String> {
    let file: File = toml::from_str(text).map_err(|error| error.to_string())?;
    if name != format!("{}.toml", file.name) {
        return Err(format!("the file name must be {}.toml", file.name));
    }
    let measured = [
        file.comments.is_some(),
        file.sloc.is_some(),
        file.units.is_some(),
        file.decisions.is_some(),
        file.clones.is_some(),
    ];
    match file.tier {
        1 if measured.iter().all(|present| *present) => Ok(file),
        2 if measured.iter().all(|present| !present) && file.statement_parents.is_none() => {
            Ok(file)
        }
        1 => Err("a tier-1 file needs comments, sloc, units, decisions and clones".to_owned()),
        2 => Err("a tier-2 file holds no measurement tables".to_owned()),
        tier => Err(format!("tier {tier} is not 1 or 2")),
    }
}

fn render(files: &[File]) -> String {
    let mut text = String::from(HEADER);
    text.push_str("pub(crate) static TABLES: [Table; ");
    write_line(&mut text, &format!("{}] = [", files.len()));
    for file in files {
        text.push_str(&table(file));
    }
    text.push_str("];\n\n");
    text.push_str(&extensions(files));
    text.push_str(&probe_roots(files));
    for file in files {
        text.push_str(&measured(file));
    }
    text
}

// Only the tests read the probe's root kind: it shows that each grammar sits in its own slot.
fn probe_roots(files: &[File]) -> String {
    let roots: Vec<String> = files.iter().map(|file| literal(&file.probe_root)).collect();
    format!(
        "\n#[cfg(test)]\npub(crate) static PROBE_ROOTS: [&str; {}] = [{}];\n",
        roots.len(),
        roots.join(", ")
    )
}

const HEADER: &str = "\
//! The language tables. `cargo xtask generate` writes this file from crates/syntax/languages/.

use crate::Language;
use crate::rules::{BindingRule, Cond, NameRule, OwnerRule, SkipRule};
use crate::tables::{
    CloneTable, DecisionTable, LiteralClass, Measured, SlocRule, Table, Tier, UnitTable,
};

";

fn table(file: &File) -> String {
    let tier = if file.tier == 1 { "One" } else { "Two" };
    let measured = if file.tier == 1 {
        format!("Some(&{})", file.name.to_uppercase())
    } else {
        "None".to_owned()
    };
    format!(
        "Table {{ name: {}, tier: Tier::{tier}, extensions: {}, probe: {}, \
         measured: {measured} }},\n",
        literal(&file.name),
        names(&file.extensions),
        literal(&file.probe),
    )
}

fn extensions(files: &[File]) -> String {
    let mut pairs: Vec<(&str, String)> = files
        .iter()
        .flat_map(|file| {
            let variant = variant(&file.name);
            file.extensions
                .iter()
                .map(move |suffix| (suffix.as_str(), variant.clone()))
        })
        .collect();
    pairs.sort();
    let mut text =
        String::from("/// Every file suffix that selects a language, sorted by suffix.\n");
    write_line(
        &mut text,
        &format!(
            "pub const EXTENSIONS: [(&str, Language); {}] = [",
            pairs.len()
        ),
    );
    for (suffix, variant) in pairs {
        write_line(
            &mut text,
            &format!("({}, Language::{variant}),", literal(suffix)),
        );
    }
    text.push_str("];\n");
    text
}

fn measured(file: &File) -> String {
    let (Some(comments), Some(sloc), Some(units), Some(decisions), Some(clones)) = (
        &file.comments,
        &file.sloc,
        &file.units,
        &file.decisions,
        &file.clones,
    ) else {
        return String::new();
    };
    format!(
        "\nstatic {}: Measured = Measured {{ comments: {}, sloc: SlocRule::{sloc:?}, \
         statement_parents: {}, units: {}, decisions: {}, clones: {} }};\n",
        file.name.to_uppercase(),
        names(comments),
        optional(file.statement_parents.as_deref()),
        unit_table(units),
        decision_table(decisions),
        clone_table(clones),
    )
}

fn unit_table(units: &UnitTable) -> String {
    format!(
        "UnitTable {{ units: {}, scopes: {}, bound: {}, descend: {}, name: NameRule::{:?}, \
         binding: {}, owner: {} }}",
        names(&units.units),
        names(&units.scopes),
        names(&units.bound),
        optional(units.descend.as_deref()),
        units.name,
        variant_of("BindingRule", units.binding.as_ref()),
        variant_of("OwnerRule", units.owner.as_ref()),
    )
}

fn decision_table(decisions: &DecisionTable) -> String {
    let conditional: Vec<String> = decisions
        .conditional
        .iter()
        .map(|(kind, cond)| format!("({}, Cond::{cond:?})", literal(kind)))
        .collect();
    format!(
        "DecisionTable {{ points: {}, conditional: &[{}], field_operators: {}, \
         token_operators: {} }}",
        names(&decisions.points),
        conditional.join(", "),
        pairs(&decisions.field_operators),
        pairs(&decisions.token_operators),
    )
}

fn clone_table(clones: &CloneTable) -> String {
    let literals: Vec<String> = clones
        .literals
        .iter()
        .map(|(class, kinds)| format!("(LiteralClass::{class:?}, {})", names(kinds)))
        .collect();
    format!(
        "CloneTable {{ functions: {}, blocks: {}, skip: {}, identifiers: {}, indent_blocks: {}, \
         literals: &[{}] }}",
        names(&clones.functions),
        names(&clones.blocks),
        variant_of("SkipRule", clones.skip.as_ref()),
        names(&clones.identifiers),
        names(&clones.indent_blocks),
        literals.join(", "),
    )
}

fn pairs(map: &BTreeMap<String, Vec<String>>) -> String {
    let entries: Vec<String> = map
        .iter()
        .map(|(kind, values)| format!("({}, {})", literal(kind), names(values)))
        .collect();
    format!("&[{}]", entries.join(", "))
}

fn names(values: &[String]) -> String {
    let quoted: Vec<String> = values.iter().map(|value| literal(value)).collect();
    format!("&[{}]", quoted.join(", "))
}

fn optional(values: Option<&[String]>) -> String {
    values.map_or_else(
        || "None".to_owned(),
        |values| format!("Some({})", names(values)),
    )
}

fn variant_of(ty: &str, value: Option<&impl std::fmt::Debug>) -> String {
    value.map_or_else(
        || "None".to_owned(),
        |value| format!("Some({ty}::{value:?})"),
    )
}

// A language name as its `Language` variant spells it: `csharp` is `Csharp`.
fn variant(name: &str) -> String {
    let mut chars = name.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn write_line(text: &mut String, line: &str) {
    writeln!(text, "{line}").expect("a String takes every write");
}

#[cfg(test)]
mod tests {
    use super::{module, parse, read, variant};
    use crate::repo;

    #[test]
    fn the_language_files_parse_with_no_problem() {
        let files = read(&repo::root().unwrap()).unwrap();
        assert_eq!(files.len(), 22);
        assert!(module(&files).is_ok());
    }

    #[test]
    fn a_file_name_must_match_its_language() {
        let text = "name = \"go\"\ntier = 2\nextensions = []\nprobe = \"\"\nprobe_root = \"\"\n";
        assert!(parse("go.toml", text).is_ok());
        assert!(parse("rust.toml", text).unwrap_err().contains("go.toml"));
    }

    #[test]
    fn a_tier_one_file_needs_its_tables_and_an_unknown_rule_fails() {
        let tier_one =
            "name = \"go\"\ntier = 1\nextensions = []\nprobe = \"\"\nprobe_root = \"\"\n";
        assert!(parse("go.toml", tier_one).unwrap_err().contains("tier-1"));
        let unknown = "name = \"go\"\ntier = 2\nextensions = []\nprobe = \"\"\nprobe_root = \"\"\n\
                       sloc = \"lines\"\n";
        assert!(parse("go.toml", unknown).is_err());
        let extra =
            "name = \"go\"\ntier = 2\nextensions = []\nprobe = \"\"\nprobe_root = \"\"\nx = 1\n";
        assert!(parse("go.toml", extra).is_err());
    }

    #[test]
    fn a_language_name_becomes_its_variant() {
        assert_eq!(variant("csharp"), "Csharp");
        assert_eq!(variant("c"), "C");
    }
}
