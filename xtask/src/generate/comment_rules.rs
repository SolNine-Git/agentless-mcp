//! The comment rule file: its checks, and the Rust table made from it.

use serde_json::{Map, Value};

use super::literal::{literal, unspaced_literal};
use super::rules::{LanguageRow, language_rows, object, text};

const ROW_KEYS: [&str; 4] = ["language", "runs", "docs", "suppressions"];
const GAP_KEY: &str = "gap";
const RUN_KEYS: [&str; 6] = ["line", "block", "doc", "head", "directive", "banner"];
const DOCSTRING_KEYS: [&str; 3] = ["style", "private", "sections"];
const JSDOC_KEYS: [&str; 3] = ["style", "tags", "aliases"];
const RUST_ITEMS_KEYS: [&str; 6] = ["style", "item", "public", "export", "facing", "sections"];
const SUPPRESSION_KEYS: [&str; 2] = ["on", "pattern"];

// Each name that the file spells, and the Rust variant it stands for.
const GAPS: [(&str, &str); 4] = [
    ("follow_up", "FollowUp"),
    ("doc_attachment", "DocAttachment"),
    ("tier_two", "TierTwo"),
    ("data_language", "DataLanguage"),
];
const HEADS: [(&str, &str); 2] = [("comment", "Comment"), ("first_line", "FirstLine")];
const TARGETS: [(&str, &str); 2] = [("comment", "Comment"), ("attribute", "Attribute")];

/// Parses the rule file into one record per language, in name order, or gives each problem.
pub fn parse(file: &str, text: &str) -> Result<Vec<LanguageRow>, Vec<String>> {
    language_rows(file, text, row)
}

fn row(entry: &Value) -> Result<LanguageRow, String> {
    let map = object(entry, &ROW_KEYS)?;
    let language = text(map, "language")?;
    let runs = declared(map, "runs", runs)?;
    let docs = declared(map, "docs", docs)?;
    let suppressions = declared(map, "suppressions", suppressions)?;
    let runs_gap = is_gap(&map["runs"]);
    if runs_gap && !(is_gap(&map["docs"]) && is_gap(&map["suppressions"])) {
        return Err(
            "docs and suppressions rows need runs rows, which name the comment forms".into(),
        );
    }
    let record = format!(
        "CommentRules {{ language: {}, runs: {runs}, docs: {docs}, suppressions: {suppressions} }}",
        literal(language)
    );
    Ok(LanguageRow {
        language: language.to_owned(),
        record,
    })
}

fn is_gap(value: &Value) -> bool {
    value.get(GAP_KEY).is_some()
}

// A group holds its rules, or exactly one key that names the reason it has none.
fn declared(
    map: &Map<String, Value>,
    group: &str,
    rules: fn(&Value) -> Result<String, String>,
) -> Result<String, String> {
    let value = &map[group];
    let made = if is_gap(value) {
        object(value, &[GAP_KEY])
            .and_then(|gap| named(gap, GAP_KEY, &GAPS))
            .map(|variant| format!("Declared::Gap(CommentGap::{variant})"))
    } else {
        rules(value).map(|rules| format!("Declared::Rules({rules})"))
    };
    made.map_err(|problem| format!("{group} {problem}"))
}

fn runs(value: &Value) -> Result<String, String> {
    let map = object(value, &RUN_KEYS)?;
    Ok(format!(
        "RunRules {{ line: {}, block: {}, doc: {}, head: Head::{}, directive: {}, banner: {} }}",
        pattern(map, "line")?,
        optional(map, "block")?,
        optional(map, "doc")?,
        named(map, "head", &HEADS)?,
        optional(map, "directive")?,
        pattern(map, "banner")?
    ))
}

fn docs(value: &Value) -> Result<String, String> {
    match value.get("style").and_then(Value::as_str) {
        Some("docstring") => {
            let map = object(value, &DOCSTRING_KEYS)?;
            let (private, sections) = (pattern(map, "private")?, pattern(map, "sections")?);
            Ok(format!(
                "DocRules::Docstring {{ private: {private}, sections: {sections} }}"
            ))
        }
        Some("jsdoc") => {
            let map = object(value, &JSDOC_KEYS)?;
            let (tags, aliases) = (pattern(map, "tags")?, aliases(&map["aliases"])?);
            Ok(format!(
                "DocRules::Jsdoc {{ tags: {tags}, aliases: &[{aliases}] }}"
            ))
        }
        Some("rust_items") => rust_items(object(value, &RUST_ITEMS_KEYS)?),
        _ => Err("key \"style\" must be \"docstring\", \"jsdoc\" or \"rust_items\"".to_owned()),
    }
}

fn rust_items(map: &Map<String, Value>) -> Result<String, String> {
    let (item, facing) = (pattern(map, "item")?, pattern(map, "facing")?);
    let sections = pattern(map, "sections")?;
    let public = literal(text(map, "public")?);
    let export = literal(text(map, "export")?);
    Ok(format!(
        "DocRules::RustItems {{ item: {item}, public: {public}, export: {export}, facing: \
         {facing}, sections: {sections} }}"
    ))
}

fn suppressions(value: &Value) -> Result<String, String> {
    let map = object(value, &SUPPRESSION_KEYS)?;
    Ok(format!(
        "SuppressionRules {{ on: Suppressed::{}, pattern: {} }}",
        named(map, "on", &TARGETS)?,
        pattern(map, "pattern")?
    ))
}

fn pattern(map: &Map<String, Value>, key: &str) -> Result<String, String> {
    text(map, key).map(unspaced_literal)
}

fn optional(map: &Map<String, Value>, key: &str) -> Result<String, String> {
    if map[key].is_null() {
        return Ok("None".to_owned());
    }
    pattern(map, key)
        .map(|pattern| format!("Some({pattern})"))
        .map_err(|problem| format!("{problem}, or null"))
}

fn named(
    map: &Map<String, Value>,
    key: &str,
    table: &[(&str, &'static str)],
) -> Result<&'static str, String> {
    let spelled = map[key].as_str();
    let found = table.iter().find(|(name, _)| Some(*name) == spelled);
    found.map(|(_, variant)| *variant).ok_or_else(|| {
        let names: Vec<&str> = table.iter().map(|(name, _)| *name).collect();
        format!("key {key:?} must be one of {names:?}")
    })
}

// The spellings sort as JSON object keys do, so the table has one order.
fn aliases(value: &Value) -> Result<String, String> {
    let problem = || "key \"aliases\" must map non-empty strings to non-empty strings".to_owned();
    let Value::Object(map) = value else {
        return Err(problem());
    };
    let pairs: Option<Vec<String>> = map
        .iter()
        .map(|(spelling, name)| {
            let name = name.as_str().filter(|name| !name.is_empty())?;
            let spelling = Some(spelling.as_str()).filter(|spelling| !spelling.is_empty())?;
            Some(format!("({}, {})", literal(spelling), literal(name)))
        })
        .collect();
    pairs.map(|pairs| pairs.join(", ")).ok_or_else(problem)
}

/// Returns the Rust module that holds the comment rules as a static table.
pub fn module(rows: &[LanguageRow]) -> String {
    let count = rows.len();
    let records = rows.iter().map(|row| row.record.as_str());
    let table = records.collect::<Vec<_>>().join(",\n");
    let uses = "CommentGap, CommentRules, Declared, DocRules, Head, RunRules, Suppressed, \
                SuppressionRules";
    format!(
        "//! The comment rules, one record per language, in name order.\n\
         //! `cargo xtask generate` writes this file from the prompt JSON files.\n\n\
         use crate::comment_rule::{{{uses}}};\n\n\
         pub(crate) static COMMENT_RULES: [CommentRules; {count}] = [{table}];\n"
    )
}

#[cfg(test)]
mod tests {
    use super::super::rules::assert_named;
    use super::{module, parse};

    const FILE: &str = "crates/prompts/data/comment_rules.json";
    const GAPS: &str = "\"runs\": {\"gap\": \"follow_up\"}, \"docs\": {\"gap\": \"tier_two\"}, \
                        \"suppressions\": {\"gap\": \"data_language\"}";

    fn file(rows: &[String]) -> String {
        format!("{{\"languages\": [{}]}}", rows.join(", "))
    }

    fn gaps(language: &str) -> String {
        format!("{{\"language\": \"{language}\", {GAPS}}}")
    }

    fn runs() -> &'static str {
        "{\"line\": \"^#\", \"block\": null, \"doc\": null, \"head\": \"comment\", \
         \"directive\": \"^#!\", \"banner\": \"^# ===\"}"
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
        says(
            "{\"languages\": [3]}",
            &["language 0: must be a JSON object"],
        );
    }

    #[test]
    fn a_row_must_hold_each_group_or_its_gap() {
        let missing = "{\"language\": \"go\", \"runs\": {\"gap\": \"follow_up\"}}";
        let words = ["language 0", "missing [\"docs\", \"suppressions\"]"];
        says(&file(&[missing.to_owned()]), &words);
        let unknown = gaps("go").replace("follow_up", "later");
        says(
            &file(&[unknown]),
            &["language 0: runs key \"gap\" must be one of"],
        );
        let extra = gaps("go").replace("\"tier_two\"}", "\"tier_two\", \"x\": 1}");
        says(
            &file(&[extra]),
            &["docs does not hold exactly the keys [\"gap\"]"],
        );
    }

    #[test]
    fn docs_and_suppressions_rows_need_runs_rows() {
        let docs = "{\"style\": \"docstring\", \"private\": \"^_\", \"sections\": \"x\"}";
        let row = gaps("go").replace("{\"gap\": \"tier_two\"}", docs);
        says(
            &file(&[row]),
            &["docs and suppressions rows need runs rows"],
        );
    }

    #[test]
    fn a_group_with_a_bad_field_is_named() {
        let in_runs = |rules: String| gaps("py").replace("{\"gap\": \"follow_up\"}", &rules);
        let blank = in_runs(runs().replace("\"^#\"", "\"\""));
        says(
            &file(&[blank]),
            &["runs key \"line\" must be a non-empty string"],
        );
        let head = in_runs(runs().replace("\"comment\"", "\"all\""));
        let words = ["runs key \"head\" must be one of [\"comment\", \"first_line\"]"];
        says(&file(&[head]), &words);
        let block = in_runs(runs().replace("\"block\": null", "\"block\": 1"));
        let words = ["key \"block\" must be a non-empty string, or null"];
        says(&file(&[block]), &words);
        let style = gaps("py").replace("{\"gap\": \"tier_two\"}", "{\"style\": \"rst\"}");
        says(&file(&[style]), &["docs key \"style\" must be"]);
    }

    #[test]
    fn languages_must_come_in_name_order_once() {
        let words = ["\"bash\" must come after \"go\""];
        says(&file(&[gaps("go"), gaps("bash")]), &words);
        says(
            &file(&[gaps("go"), gaps("go")]),
            &["\"go\" must come after \"go\""],
        );
    }

    #[test]
    fn each_language_makes_one_record_in_file_order() {
        let jsdoc = "{\"style\": \"jsdoc\", \"tags\": \"@(x)\", \"aliases\": {\"y\": \"x\"}}";
        let suppressions = "{\"on\": \"comment\", \"pattern\": \"noqa\"}";
        let python = gaps("python")
            .replace("{\"gap\": \"follow_up\"}", runs())
            .replace("{\"gap\": \"tier_two\"}", jsdoc)
            .replace("{\"gap\": \"data_language\"}", suppressions);
        let rows = parse(FILE, &file(&[gaps("bash"), python])).unwrap();
        let names: Vec<&str> = rows.iter().map(|row| row.language.as_str()).collect();
        assert_eq!(names, ["bash", "python"]);
        let source = module(&rows);
        let wanted = [
            "pub(crate) static COMMENT_RULES: [CommentRules; 2] = [",
            "runs: Declared::Gap(CommentGap::FollowUp)",
            "block: None, doc: None, head: Head::Comment",
            "DocRules::Jsdoc { tags: \"@(x)\", aliases: &[(\"y\", \"x\")] }",
            "SuppressionRules { on: Suppressed::Comment, pattern: \"noqa\" }",
        ];
        for text in wanted {
            assert!(source.contains(text), "{text}: {source}");
        }
    }

    #[test]
    fn a_rust_items_row_makes_its_record() {
        let docs = "{\"style\": \"rust_items\", \"item\": \"^(?P<name>\\\\w+)\", \"public\": \
                    \"pub\", \"export\": \"macro_export\", \"facing\": \"derive\", \"sections\": \
                    \"# Args\"}";
        let suppressions = "{\"on\": \"attribute\", \"pattern\": \"^allow\"}";
        let rust = gaps("rust")
            .replace("{\"gap\": \"follow_up\"}", runs())
            .replace("{\"gap\": \"tier_two\"}", docs)
            .replace("{\"gap\": \"data_language\"}", suppressions);
        let source = module(&parse(FILE, &file(&[rust])).unwrap());
        let wanted = [
            "DocRules::RustItems { item: \"^(?P<name>\\\\w+)\", public: \"pub\", export: \
             \"macro_export\", facing: \"derive\", sections: \"# Args\" }",
            "SuppressionRules { on: Suppressed::Attribute, pattern: \"^allow\" }",
        ];
        for text in wanted {
            assert!(source.contains(text), "{text}: {source}");
        }
        let missing = docs.replace(", \"export\": \"macro_export\"", "");
        let row = gaps("rust")
            .replace("{\"gap\": \"follow_up\"}", runs())
            .replace("{\"gap\": \"tier_two\"}", &missing);
        says(&file(&[row]), &["docs", "missing [\"export\"]"]);
    }
}
