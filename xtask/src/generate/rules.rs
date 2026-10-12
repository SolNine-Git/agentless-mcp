//! The contract rule file: its checks, and the Rust table made from it.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use super::literal::literal;

const TOP_KEYS: [&str; 1] = ["rules"];
const RULE_KEYS: [&str; 5] = ["id", "languages", "severity", "query", "message"];

/// One contract rule for one language. `severity` is the name of a `Severity` variant.
#[derive(Debug)]
pub struct Rule {
    pub id: String,
    pub language: String,
    pub severity: &'static str,
    pub query: String,
    pub message: String,
}

/// Parses the rule file into one record per rule and language, in order, or gives each problem.
pub fn parse(file: &str, text: &str) -> Result<Vec<Rule>, Vec<String>> {
    let document: Value = serde_json::from_str(text)
        .map_err(|error| vec![format!("{file}: the file is not valid JSON: {error}")])?;
    let entries = entries(&document).map_err(|problem| vec![format!("{file}: {problem}")])?;
    let mut rules = Vec::new();
    let mut problems = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        match records(entry) {
            Ok(records) => rules.extend(records),
            Err(problem) => problems.push(format!("{file}: rule {index}: {problem}")),
        }
    }
    problems.extend(
        duplicates(&rules)
            .into_iter()
            .map(|problem| format!("{file}: {problem}")),
    );
    if problems.is_empty() {
        Ok(rules)
    } else {
        Err(problems)
    }
}

fn entries(document: &Value) -> Result<&Vec<Value>, String> {
    let top = object(document, &TOP_KEYS).map_err(|problem| format!("the top level {problem}"))?;
    match &top["rules"] {
        Value::Array(entries) if !entries.is_empty() => Ok(entries),
        _ => Err("key \"rules\" must be a non-empty list".to_owned()),
    }
}

fn object<'v>(value: &'v Value, keys: &[&str]) -> Result<&'v Map<String, Value>, String> {
    let Value::Object(map) = value else {
        return Err("must be a JSON object".to_owned());
    };
    let missing: Vec<&str> = keys
        .iter()
        .copied()
        .filter(|key| !map.contains_key(*key))
        .collect();
    let unknown: Vec<&str> = map
        .keys()
        .map(String::as_str)
        .filter(|key| !keys.contains(key))
        .collect();
    if missing.is_empty() && unknown.is_empty() {
        Ok(map)
    } else {
        Err(format!(
            "does not hold exactly the keys {keys:?}: missing {missing:?}, unknown {unknown:?}"
        ))
    }
}

fn records(entry: &Value) -> Result<Vec<Rule>, String> {
    let map = object(entry, &RULE_KEYS)?;
    let id = text(map, "id")?;
    let query = text(map, "query")?;
    let message = text(map, "message")?;
    let severity = severity(&map["severity"])?;
    let languages = languages(&map["languages"])?;
    Ok(languages
        .into_iter()
        .map(|language| Rule {
            id: id.to_owned(),
            language,
            severity,
            query: query.to_owned(),
            message: message.to_owned(),
        })
        .collect())
}

fn text<'m>(map: &'m Map<String, Value>, key: &str) -> Result<&'m str, String> {
    map[key]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| format!("key {key:?} must be a non-empty string"))
}

fn severity(value: &Value) -> Result<&'static str, String> {
    match value.as_str() {
        Some("warning") => Ok("Warning"),
        Some("advisory") => Ok("Advisory"),
        _ => Err("key \"severity\" must be \"warning\" or \"advisory\"".to_owned()),
    }
}

fn languages(value: &Value) -> Result<Vec<String>, String> {
    let problem =
        || "key \"languages\" must be a non-empty list of distinct non-empty strings".to_owned();
    let list = value
        .as_array()
        .filter(|list| !list.is_empty())
        .ok_or_else(problem)?;
    let names: Vec<String> = list
        .iter()
        .map(|name| {
            name.as_str()
                .filter(|name| !name.trim().is_empty())
                .map(str::to_owned)
        })
        .collect::<Option<_>>()
        .ok_or_else(problem)?;
    let distinct: BTreeSet<&String> = names.iter().collect();
    if distinct.len() == names.len() {
        Ok(names)
    } else {
        Err(problem())
    }
}

fn duplicates(rules: &[Rule]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    rules
        .iter()
        .filter(|rule| !seen.insert((rule.id.as_str(), rule.language.as_str())))
        .map(|rule| format!("rule {:?} is defined twice for {}", rule.id, rule.language))
        .collect()
}

/// Returns the Rust module that holds the rules as a static table.
pub fn module(rules: &[Rule]) -> String {
    let records: Vec<String> = rules.iter().map(record).collect();
    format!(
        "//! The contract rules, one record per rule and language, in file order.\n\
         //! `cargo xtask generate` writes this file from the prompt JSON files.\n\n\
         use crate::rule::{{ContractRule, Severity}};\n\n\
         pub(crate) static RULES: [ContractRule; {}] = [{}];\n",
        rules.len(),
        records.join(",\n")
    )
}

fn record(rule: &Rule) -> String {
    format!(
        "ContractRule {{ id: {}, language: {}, severity: Severity::{}, query: {}, message: {} }}",
        literal(&rule.id),
        literal(&rule.language),
        rule.severity,
        literal(&rule.query),
        literal(&rule.message)
    )
}

#[cfg(test)]
mod tests {
    use super::{module, parse};

    const FILE: &str = "src/agentless_mcp/prompts/contract_rules.json";

    fn rule(fields: &str) -> String {
        format!("{{\"rules\": [{{{fields}}}]}}")
    }

    fn valid(id: &str, languages: &str) -> String {
        format!(
            "\"id\": \"{id}\", \"languages\": {languages}, \"severity\": \"warning\", \
             \"query\": \"(x) @y\", \"message\": \"m\""
        )
    }

    fn says(text: &str, words: &[&str]) {
        let problems = parse(FILE, text).unwrap_err();
        assert!(
            problems.iter().any(|problem| problem.starts_with(FILE)
                && words.iter().all(|word| problem.contains(word))),
            "{words:?}: {problems:?}"
        );
    }

    #[test]
    fn text_that_is_not_json_names_the_file() {
        says("{\"rules\": [", &["not valid JSON"]);
    }

    #[test]
    fn the_top_level_must_hold_exactly_the_rules_key() {
        says("[]", &["the top level must be a JSON object"]);
        says("{}", &["the top level", "missing [\"rules\"]"]);
        says(
            &format!(
                "{{\"rules\": [{{{}}}], \"extra\": 1}}",
                valid("a", "[\"go\"]")
            ),
            &["unknown [\"extra\"]"],
        );
    }

    #[test]
    fn the_rules_key_must_hold_a_non_empty_list() {
        says(
            "{\"rules\": []}",
            &["key \"rules\" must be a non-empty list"],
        );
        says(
            "{\"rules\": {}}",
            &["key \"rules\" must be a non-empty list"],
        );
    }

    #[test]
    fn a_rule_must_hold_exactly_the_five_keys() {
        says("{\"rules\": [3]}", &["rule 0: must be a JSON object"]);
        let missing = rule(
            "\"id\": \"a\", \"languages\": [\"go\"], \"severity\": \"warning\", \"query\": \"q\"",
        );
        says(&missing, &["rule 0", "missing [\"message\"]"]);
        let unknown = rule(&format!("{}, \"notes\": \"n\"", valid("a", "[\"go\"]")));
        says(&unknown, &["rule 0", "unknown [\"notes\"]"]);
    }

    #[test]
    fn a_blank_or_non_text_field_is_named() {
        says(
            &rule(&valid(" ", "[\"go\"]")),
            &["rule 0", "key \"id\" must be a non-empty string"],
        );
        let query = valid("a", "[\"go\"]").replace("\"(x) @y\"", "7");
        says(
            &rule(&query),
            &["rule 0", "key \"query\" must be a non-empty string"],
        );
        let message = valid("a", "[\"go\"]").replace("\"m\"", "\"\"");
        says(
            &rule(&message),
            &["rule 0", "key \"message\" must be a non-empty string"],
        );
    }

    #[test]
    fn a_severity_other_than_warning_or_advisory_is_named() {
        let severity = valid("a", "[\"go\"]").replace("\"warning\"", "\"error\"");
        says(&rule(&severity), &["rule 0", "key \"severity\" must be"]);
    }

    #[test]
    fn languages_must_be_distinct_non_empty_strings() {
        for languages in ["[]", "\"go\"", "[\"go\", \"go\"]", "[\"go\", \" \"]", "[1]"] {
            says(
                &rule(&valid("a", languages)),
                &["rule 0", "key \"languages\" must be"],
            );
        }
    }

    #[test]
    fn one_rule_twice_for_a_language_is_named() {
        let text = format!(
            "{{\"rules\": [{{{}}}, {{{}}}]}}",
            valid("a", "[\"go\", \"rust\"]"),
            valid("a", "[\"rust\"]")
        );
        says(&text, &["rule \"a\" is defined twice for rust"]);
    }

    #[test]
    fn each_language_makes_one_record_in_file_order() {
        let text = format!(
            "{{\"rules\": [{{{}}}, {{{}}}]}}",
            valid("a", "[\"go\", \"rust\"]"),
            valid("b", "[\"lua\"]")
        );
        let rules = parse(FILE, &text).unwrap();
        let pairs: Vec<(&str, &str)> = rules
            .iter()
            .map(|rule| (rule.id.as_str(), rule.language.as_str()))
            .collect();
        assert_eq!(pairs, [("a", "go"), ("a", "rust"), ("b", "lua")]);
        let source = module(&rules);
        assert!(source.contains("pub(crate) static RULES: [ContractRule; 3] = ["));
        assert!(source.contains("severity: Severity::Warning, query: \"(x) @y\""));
    }
}
