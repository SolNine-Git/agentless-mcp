//! The description files: plain texts keyed by name, and the Rust modules made from them.

use super::templates::{WRITTEN_BY, entry_text, keyed, text_const};

/// One description: its key and its text.
pub type Description = (String, String);

/// Parses one description file, sorted by key, or returns each problem with the file and the key.
pub fn parse(file: &str, text: &str) -> Result<Vec<Description>, Vec<String>> {
    keyed(file, text, |key, value| {
        Ok((key.to_owned(), entry_text(key, value)?.to_owned()))
    })
}

/// Returns the Rust module that holds one file's descriptions as consts, each with the doc line
/// that `doc` makes from its key.
pub fn module(summary: &str, doc: impl Fn(&str) -> String, descriptions: &[Description]) -> String {
    let items: Vec<String> = descriptions
        .iter()
        .map(|(key, text)| text_const(&doc(key), key, text))
        .collect();
    format!("//! {summary}\n{WRITTEN_BY}\n\n{}", items.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::{module, parse};

    const FILE: &str = "src/agentless_mcp/prompts/tool_descriptions.json";

    fn names_file_and_key(text: &str, key: &str, says: &str) {
        let problems = parse(FILE, text).unwrap_err();
        let key = format!("key \"{key}\"");
        assert!(
            problems.iter().any(|problem| problem.starts_with(FILE)
                && problem.contains(&key)
                && problem.contains(says)),
            "{key} {says}: {problems:?}"
        );
    }

    #[test]
    fn text_that_is_not_a_json_object_names_the_file() {
        let problems = parse(FILE, "{\"a\": ").unwrap_err();
        assert!(problems[0].starts_with(FILE) && problems[0].contains("not valid JSON"));
        assert_eq!(
            parse(FILE, "[\"a\"]").unwrap_err(),
            [format!("{FILE}: the top level must be a JSON object")]
        );
    }

    #[test]
    fn a_bad_key_and_an_empty_value_are_named() {
        names_file_and_key("{\"Bad-Key\": \"x\"}", "Bad-Key", "lowercase identifier");
        for value in ["3", "\" \\n\"", "null"] {
            names_file_and_key(&format!("{{\"a\": {value}}}"), "a", "non-empty string");
        }
    }

    #[test]
    fn descriptions_come_in_key_order_and_a_brace_is_plain_text() {
        let descriptions = parse(FILE, "{\"b\": \"x {y} }\", \"a\": \"z\"}").unwrap();
        let expected = [("a", "z"), ("b", "x {y} }")].map(|(key, text)| (key.into(), text.into()));
        assert_eq!(descriptions, expected);
    }

    #[test]
    fn the_module_declares_each_description_with_its_doc_line() {
        let text = "word ".repeat(40);
        let descriptions = parse(FILE, &format!("{{\"orient\": \"{}\"}}", text.trim_end()))
            .expect("the file parses");
        let made = module(
            "The tool descriptions.",
            |key| format!("The description of the `{key}` tool."),
            &descriptions,
        );
        assert!(made.starts_with("//! The tool descriptions.\n//! `cargo xtask generate`"));
        assert!(made.contains(
            "/// The description of the `orient` tool.\npub const ORIENT: &str = \"word word"
        ));
        let long = made.lines().find(|line| line.chars().count() > 100);
        assert_eq!(long, None, "{made}");
    }
}
