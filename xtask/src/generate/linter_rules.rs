//! The linter rule file: its checks, and the Rust table made from it.

use serde_json::{Map, Value};

use super::literal::literal;
use super::rules::{LanguageRow, language_rows, object, text};

const LINTER_KEYS: [&str; 8] = [
    "name",
    "local",
    "program",
    "args",
    "configs",
    "manifest",
    "table",
    "syntax_code",
];

// Each gap name that the file spells, and the Rust variant it stands for.
const GAPS: [(&str, &str); 3] = [
    ("follow_up", "FollowUp"),
    ("tier_two", "TierTwo"),
    ("data_language", "DataLanguage"),
];

/// Parses the rule file into one record per language, in name order, or gives each problem.
pub fn parse(file: &str, text: &str) -> Result<Vec<LanguageRow>, Vec<String>> {
    language_rows(file, text, row)
}

// A row holds its linter, or one key that names the reason it has none.
fn row(entry: &Value) -> Result<LanguageRow, String> {
    let declared = if entry.get("gap").is_some() {
        "gap"
    } else {
        "linter"
    };
    let map = object(entry, &["language", declared])?;
    let language = text(map, "language")?;
    let linter = if declared == "gap" {
        format!("Err(LinterGap::{})", gap(map)?)
    } else {
        let linter = linter(&map["linter"]).map_err(|problem| format!("linter {problem}"))?;
        format!("Ok({linter})")
    };
    let record = format!(
        "LinterRules {{ language: {}, linter: {linter} }}",
        literal(language)
    );
    Ok(LanguageRow {
        language: language.to_owned(),
        record,
    })
}

fn gap(map: &Map<String, Value>) -> Result<&'static str, String> {
    let spelled = map["gap"].as_str();
    let found = GAPS.iter().find(|(name, _)| Some(*name) == spelled);
    found.map(|(_, variant)| *variant).ok_or_else(|| {
        let names: Vec<&str> = GAPS.iter().map(|(name, _)| *name).collect();
        format!("key \"gap\" must be one of {names:?}")
    })
}

fn linter(value: &Value) -> Result<String, String> {
    let map = object(value, &LINTER_KEYS)?;
    let field = |key: &str| text(map, key).map(literal);
    let args = list(map, "args")?;
    if !args.iter().any(|arg| arg.contains("{path}")) {
        return Err("key \"args\" must name the file as {path}".to_owned());
    }
    Ok(format!(
        "Linter {{ name: {}, local: {}, program: {}, args: &[{}], configs: &[{}], manifest: {}, \
         table: &[{}], syntax_code: {} }}",
        field("name")?,
        field("local")?,
        field("program")?,
        literals(&args),
        literals(&list(map, "configs")?),
        field("manifest")?,
        literals(&list(map, "table")?),
        field("syntax_code")?
    ))
}

fn list<'m>(map: &'m Map<String, Value>, key: &str) -> Result<Vec<&'m str>, String> {
    let problem = || format!("key {key:?} must be a non-empty list of non-empty strings");
    let items = map[key]
        .as_array()
        .filter(|items| !items.is_empty())
        .ok_or_else(problem)?;
    items
        .iter()
        .map(|item| item.as_str().filter(|item| !item.is_empty()))
        .collect::<Option<Vec<&str>>>()
        .ok_or_else(problem)
}

fn literals(items: &[&str]) -> String {
    let made: Vec<String> = items.iter().map(|item| literal(item)).collect();
    made.join(", ")
}

/// Returns the Rust module that holds the linter rules as a static table.
pub fn module(rows: &[LanguageRow]) -> String {
    let count = rows.len();
    let records = rows.iter().map(|row| row.record.as_str());
    let table = records.collect::<Vec<_>>().join(",\n");
    format!(
        "//! The linter rules, one record per language, in name order.\n\
         //! `cargo xtask generate` writes this file from the prompt JSON files.\n\n\
         use crate::linter_rule::{{Linter, LinterGap, LinterRules}};\n\n\
         pub(crate) static LINTER_RULES: [LinterRules; {count}] = [{table}];\n"
    )
}

#[cfg(test)]
mod tests {
    use super::super::rules::assert_named;
    use super::{module, parse};

    const FILE: &str = "crates/prompts/data/linter_rules.json";

    fn file(rows: &[String]) -> String {
        format!("{{\"languages\": [{}]}}", rows.join(", "))
    }

    fn gap(language: &str) -> String {
        format!("{{\"language\": \"{language}\", \"gap\": \"follow_up\"}}")
    }

    fn ruff() -> String {
        "{\"name\": \"ruff\", \"local\": \".venv/bin/ruff\", \"program\": \"ruff\", \"args\": \
         [\"check\", \"{path}\"], \"configs\": [\"ruff.toml\"], \"manifest\": \"pyproject.toml\", \
         \"table\": [\"tool\", \"ruff\"], \"syntax_code\": \"invalid-syntax\"}"
            .to_owned()
    }

    fn linted(language: &str, linter: &str) -> String {
        format!("{{\"language\": \"{language}\", \"linter\": {linter}}}")
    }

    fn says(text: &str, words: &[&str]) {
        assert_named(&parse(FILE, text).unwrap_err(), FILE, words);
    }

    #[test]
    fn the_top_level_must_hold_a_non_empty_list_of_languages() {
        says("[]", &["the top level must be a JSON object"]);
        says(
            "{\"languages\": []}",
            &["key \"languages\" must be a non-empty list"],
        );
    }

    #[test]
    fn a_row_holds_its_linter_or_a_known_gap() {
        let both = gap("go").replace('}', ", \"linter\": {}}");
        says(&file(&[both]), &["language 0", "unknown [\"linter\"]"]);
        let unknown = gap("go").replace("follow_up", "later");
        says(&file(&[unknown]), &["key \"gap\" must be one of"]);
        let neither = "{\"language\": \"go\"}".to_owned();
        says(&file(&[neither]), &["missing [\"linter\"]"]);
    }

    #[test]
    fn a_linter_with_a_bad_field_is_named() {
        let missing = ruff().replace(", \"syntax_code\": \"invalid-syntax\"", "");
        let words = ["language 0: linter", "missing [\"syntax_code\"]"];
        says(&file(&[linted("python", &missing)]), &words);
        let empty = ruff().replace("[\"ruff.toml\"]", "[]");
        let words = ["key \"configs\" must be a non-empty list"];
        says(&file(&[linted("python", &empty)]), &words);
        let unnamed = ruff().replace("\"{path}\"", "\"-\"");
        let words = ["key \"args\" must name the file as {path}"];
        says(&file(&[linted("python", &unnamed)]), &words);
    }

    #[test]
    fn languages_must_come_in_name_order_once() {
        let rows = [gap("go"), gap("bash")];
        says(&file(&rows), &["\"bash\" must come after \"go\""]);
        let rows = [gap("go"), gap("go")];
        says(&file(&rows), &["\"go\" must come after \"go\""]);
    }

    #[test]
    fn each_language_makes_one_record_in_file_order() {
        let rows = parse(FILE, &file(&[gap("bash"), linted("python", &ruff())])).unwrap();
        let names: Vec<&str> = rows.iter().map(|row| row.language.as_str()).collect();
        assert_eq!(names, ["bash", "python"]);
        let source = module(&rows);
        let wanted = [
            "pub(crate) static LINTER_RULES: [LinterRules; 2] = [",
            "LinterRules { language: \"bash\", linter: Err(LinterGap::FollowUp) }",
            "linter: Ok(Linter { name: \"ruff\", local: \".venv/bin/ruff\", program: \"ruff\", \
             args: &[\"check\", \"{path}\"], configs: &[\"ruff.toml\"], manifest: \
             \"pyproject.toml\", table: &[\"tool\", \"ruff\"], syntax_code: \"invalid-syntax\" })",
        ];
        for text in wanted {
            assert!(source.contains(text), "{text}: {source}");
        }
    }
}
