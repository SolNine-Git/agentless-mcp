//! The template files: their checks, their placeholders, and the Rust modules made from them.

use std::collections::BTreeSet;

use serde_json::Value;

use super::literal::{literal, literal_within};
use super::placeholders::{self, Kind, PLACEHOLDERS};

// The prompts crate implements its Signature trait for one kind and for tuples of 2 to 6 kinds.
const MOST_VALUES: usize = 6;
const LINE: usize = 100;
/// The second line of each module that the prompt JSON files make.
pub const WRITTEN_BY: &str =
    "//! `cargo xtask generate` writes this file from the prompt JSON files.";

/// One template: its key, its text, its value names and kinds in value order, and its parts.
#[derive(Debug)]
pub struct Template {
    pub key: String,
    pub text: String,
    pub values: Vec<(&'static str, Kind)>,
    pub pieces: Vec<Piece>,
}

/// One part of a template: literal text, the position of a value, or the position of a count with
/// its form for a count of one and its form for any other count.
#[derive(Debug, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Value(usize),
    Choice(usize, String, String),
}

enum Segment<'t> {
    Literal(&'t str),
    Name(&'t str),
    Choice(&'t str, &'t str, &'t str),
}

/// Parses one template file, sorted by key, or returns each problem with the file and the key.
pub fn parse(file: &str, text: &str) -> Result<Vec<Template>, Vec<String>> {
    keyed(file, text, template)
}

/// Reads one JSON object and makes an item of each entry in key order, or returns each problem
/// with the file and the key.
pub fn keyed<T>(
    file: &str,
    text: &str,
    item: impl Fn(&str, &Value) -> Result<T, String>,
) -> Result<Vec<T>, Vec<String>> {
    let document: Value = serde_json::from_str(text)
        .map_err(|error| vec![format!("{file}: the file is not valid JSON: {error}")])?;
    let Value::Object(entries) = document else {
        return Err(vec![format!("{file}: the top level must be a JSON object")]);
    };
    let mut keys: Vec<&String> = entries.keys().collect();
    keys.sort();
    let mut items = Vec::new();
    let mut problems = Vec::new();
    for key in keys {
        match item(key, &entries[key]) {
            Ok(made) => items.push(made),
            Err(problem) => problems.push(format!("{file}: key {key:?}: {problem}")),
        }
    }
    if problems.is_empty() {
        Ok(items)
    } else {
        Err(problems)
    }
}

/// Returns the non-empty text of an entry, or the problem that refuses it.
pub fn entry_text<'v>(key: &str, value: &'v Value) -> Result<&'v str, String> {
    if !is_name(key) {
        return Err("the key is not a lowercase identifier".to_owned());
    }
    value
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| "the value must be a non-empty string".to_owned())
}

fn template(key: &str, value: &Value) -> Result<Template, String> {
    let text = entry_text(key, value)?;
    let segments = segments(text)?;
    check_choices(&segments)?;
    let values = value_order(&segments)?;
    let pieces = segments
        .iter()
        .map(|segment| piece(segment, &values))
        .collect();
    Ok(Template {
        key: key.to_owned(),
        text: text.to_owned(),
        values,
        pieces,
    })
}

fn segments(text: &str) -> Result<Vec<Segment<'_>>, String> {
    let mut segments = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find(['{', '}']) {
        if open > 0 {
            segments.push(Segment::Literal(&rest[..open]));
        }
        let field = &rest[open..];
        let close = field
            .find('}')
            .filter(|_| field.starts_with('{'))
            .ok_or("the text has a brace that is not part of a {name} placeholder")?;
        segments.push(placeholder(&field[1..close])?);
        rest = &field[close + 1..];
    }
    if !rest.is_empty() {
        segments.push(Segment::Literal(rest));
    }
    Ok(segments)
}

// A `{name}` placeholder, or a `{name:one|many}` choice of two non-empty forms.
fn placeholder(inside: &str) -> Result<Segment<'_>, String> {
    if is_name(inside) {
        return Ok(Segment::Name(inside));
    }
    let form = |text: &str| !text.is_empty() && !text.contains(['{', '|']);
    let choice = inside.split_once(':').and_then(|(name, forms)| {
        let (one, many) = forms.split_once('|')?;
        (is_name(name) && form(one) && form(many)).then_some(Segment::Choice(name, one, many))
    });
    choice.ok_or_else(|| {
        format!(
            "the field {{{inside}}} is not a plain {{name}} placeholder or a {{name:one|many}} \
             choice"
        )
    })
}

// A choice reads the count it chooses by, so that count must be a count the template prints.
fn check_choices(segments: &[Segment<'_>]) -> Result<(), String> {
    for segment in segments {
        let Segment::Choice(name, one, many) = segment else {
            continue;
        };
        let field = format!("{{{name}:{one}|{many}}}");
        let position = placeholders::position(name).ok_or_else(|| placeholders::unknown(name))?;
        if PLACEHOLDERS[position].1 != Kind::Count {
            return Err(format!(
                "the choice field {field} needs a count placeholder, and {{{name}}} takes text"
            ));
        }
        let printed = |other: &Segment<'_>| matches!(other, Segment::Name(shown) if shown == name);
        if !segments.iter().any(printed) {
            return Err(format!(
                "the choice field {field} chooses by {{{name}}}, but the template does not \
                 print {{{name}}}"
            ));
        }
    }
    Ok(())
}

fn is_name(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn value_order(segments: &[Segment<'_>]) -> Result<Vec<(&'static str, Kind)>, String> {
    let mut positions = BTreeSet::new();
    for segment in segments {
        if let Segment::Name(name) = segment {
            positions
                .insert(placeholders::position(name).ok_or_else(|| placeholders::unknown(name))?);
        }
    }
    if positions.len() > MOST_VALUES {
        return Err(format!(
            "the template has {} placeholders, and a template takes at most {MOST_VALUES}",
            positions.len()
        ));
    }
    Ok(positions
        .into_iter()
        .map(|position| PLACEHOLDERS[position])
        .collect())
}

fn piece(segment: &Segment<'_>, values: &[(&'static str, Kind)]) -> Piece {
    let position = |name: &str| {
        values
            .iter()
            .position(|(value, _)| *value == name)
            .expect("value_order lists every placeholder name")
    };
    match segment {
        Segment::Literal(text) => Piece::Text((*text).to_owned()),
        Segment::Name(name) => Piece::Value(position(name)),
        Segment::Choice(name, one, many) => {
            Piece::Choice(position(name), (*one).to_owned(), (*many).to_owned())
        }
    }
}

/// Returns the Rust module that holds one file's templates as consts.
pub fn module(summary: &str, templates: &[Template]) -> String {
    let items: Vec<String> = templates.iter().map(item).collect();
    format!(
        "//! {summary}\n{WRITTEN_BY}\n\n{}\n{}",
        imports(templates),
        items.join("\n")
    )
}

fn imports(templates: &[Template]) -> String {
    let mut names = BTreeSet::new();
    for template in templates
        .iter()
        .filter(|template| !template.values.is_empty())
    {
        names.extend(["Piece", "Template"]);
        names.extend(template.values.iter().map(|(_, kind)| kind.marker()));
    }
    if names.is_empty() {
        return String::new();
    }
    let names: Vec<&str> = names.into_iter().collect();
    format!("use crate::template::{{{}}};\n", names.join(", "))
}

fn item(template: &Template) -> String {
    let name = template.key.to_uppercase();
    if template.values.is_empty() {
        return text_const(
            "A text that takes no values.",
            &template.key,
            &template.text,
        );
    }
    let (takes, signature) = signature(&template.values);
    let pieces: Vec<String> = template.pieces.iter().map(piece_source).collect();
    format!(
        "/// A template that takes {takes}.\n\
         pub const {name}: Template<{signature}> = Template::new(&[{}]);\n",
        pieces.join(", ")
    )
}

/// Returns the declaration of one `&str` const named after `key`, with `doc` as its doc line.
pub fn text_const(doc: &str, key: &str, text: &str) -> String {
    let head = format!("pub const {}: &str = ", key.to_uppercase());
    // The first line also holds the opening quote and a closing `";` or `\`.
    let text = literal_within(text, LINE.saturating_sub(head.len() + 3));
    format!("/// {doc}\n{head}{text};\n")
}

fn signature(values: &[(&str, Kind)]) -> (String, String) {
    let names: Vec<String> = values.iter().map(|(name, _)| format!("`{name}`")).collect();
    let kinds: Vec<&str> = values.iter().map(|(_, kind)| kind.marker()).collect();
    if let ([name], [kind]) = (names.as_slice(), kinds.as_slice()) {
        return (name.clone(), (*kind).to_owned());
    }
    (
        format!("({})", names.join(", ")),
        format!("({})", kinds.join(", ")),
    )
}

fn piece_source(piece: &Piece) -> String {
    match piece {
        Piece::Text(text) => format!("Piece::Text({})", literal(text)),
        Piece::Value(position) => format!("Piece::Value({position})"),
        Piece::Choice(position, one, many) => {
            format!(
                "Piece::Choice({position}, {}, {})",
                literal(one),
                literal(many)
            )
        }
    }
}

/// Returns the test catalog module: each template's module, key, value names and parts.
pub fn catalog(files: &[(&str, &[Template])]) -> String {
    let mut entries = Vec::new();
    for (module, templates) in files {
        entries.extend(templates.iter().map(|template| entry(module, template)));
    }
    let modules: Vec<&str> = files.iter().map(|(module, _)| *module).collect();
    format!(
        "//! Each template with its value names, for the render tests.\n{WRITTEN_BY}\n\n\
         use super::{{{}}};\nuse crate::template::Piece;\n\n\
         pub(crate) const CATALOG: [(&str, &str, &[&str], &[Piece]); {}] = [{}];\n",
        modules.join(", "),
        entries.len(),
        entries.join(",\n")
    )
}

fn entry(module: &str, template: &Template) -> String {
    let name = template.key.to_uppercase();
    let names: Vec<String> = template
        .values
        .iter()
        .map(|(value, _)| format!("{value:?}"))
        .collect();
    let pieces = if template.values.is_empty() {
        format!("&[Piece::Text({module}::{name})]")
    } else {
        format!("{module}::{name}.pieces()")
    };
    format!(
        "({module:?}, {:?}, &[{}], {pieces})",
        template.key,
        names.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::{Piece, Template, catalog, module, parse};
    use crate::generate::placeholders::Kind;

    const FILE: &str = "crates/prompts/data/messages.json";

    fn problems_of(text: &str) -> Vec<String> {
        parse(FILE, text).unwrap_err()
    }

    fn names_file_and_key(problems: &[String], key: &str, says: &str) {
        let key = format!("key \"{key}\"");
        assert!(
            problems.iter().any(|problem| problem.starts_with(FILE)
                && problem.contains(&key)
                && problem.contains(says)),
            "{key} {says}: {problems:?}"
        );
    }

    #[test]
    fn text_that_is_not_json_names_the_file() {
        let problems = problems_of("{\"a\": ");
        assert!(problems[0].starts_with(FILE) && problems[0].contains("not valid JSON"));
    }

    #[test]
    fn a_top_level_that_is_not_an_object_names_the_file() {
        let problems = problems_of("[\"a\"]");
        assert_eq!(
            problems,
            [format!("{FILE}: the top level must be a JSON object")]
        );
    }

    #[test]
    fn a_key_that_is_not_a_lowercase_identifier_is_named() {
        names_file_and_key(
            &problems_of("{\"Bad-Key\": \"x\"}"),
            "Bad-Key",
            "lowercase identifier",
        );
        names_file_and_key(
            &problems_of("{\"9a\": \"x\"}"),
            "9a",
            "lowercase identifier",
        );
    }

    #[test]
    fn a_value_that_is_not_a_non_empty_string_is_named() {
        let problems = problems_of("{\"a\": 3, \"b\": \" \\n\", \"c\": null}");
        for key in ["a", "b", "c"] {
            names_file_and_key(&problems, key, "non-empty string");
        }
    }

    #[test]
    fn a_field_that_is_not_a_plain_placeholder_is_named() {
        let fields = [
            ("a", "x {} y", "{}"),
            ("b", "{0}", "{0}"),
            ("c", "{path:>5}", "{path:>5}"),
            ("d", "{path!r}", "{path!r}"),
            ("e", "{path.x}", "{path.x}"),
            ("f", "{path[0]}", "{path[0]}"),
            ("g", "{{path}}", "{{path}"),
        ];
        let text: Vec<String> = fields
            .iter()
            .map(|(key, value, _)| format!("\"{key}\": \"{value}\""))
            .collect();
        let problems = problems_of(&format!("{{{}}}", text.join(", ")));
        for (key, _, field) in fields {
            names_file_and_key(&problems, key, &format!("the field {field} is not a plain"));
        }
    }

    #[test]
    fn a_brace_outside_a_placeholder_is_named() {
        let problems = problems_of("{\"a\": \"}}\", \"b\": \"x } y\", \"c\": \"x {path\"}");
        for key in ["a", "b", "c"] {
            names_file_and_key(&problems, key, "brace");
        }
    }

    #[test]
    fn a_choice_field_on_a_text_placeholder_is_named() {
        names_file_and_key(
            &problems_of("{\"a\": \"{path} {path:one|many}\"}"),
            "a",
            "the choice field {path:one|many} needs a count placeholder, and {path} takes text",
        );
    }

    #[test]
    fn a_choice_field_with_a_malformed_body_is_named() {
        let fields = [
            ("a", "{count:key}", "{count:key}"),
            ("b", "{count:|keys}", "{count:|keys}"),
            ("c", "{count:key|}", "{count:key|}"),
            ("d", "{count:key|keys|more}", "{count:key|keys|more}"),
            ("e", "{count:key{s|keys}", "{count:key{s|keys}"),
            ("f", "{Count:key|keys}", "{Count:key|keys}"),
        ];
        let text: Vec<String> = fields
            .iter()
            .map(|(key, field, _)| format!("\"{key}\": \"{{count}} {field}\""))
            .collect();
        let problems = problems_of(&format!("{{{}}}", text.join(", ")));
        for (key, _, named) in fields {
            let says = format!(
                "the field {named} is not a plain {{name}} placeholder or a {{name:one|many}} \
                 choice"
            );
            names_file_and_key(&problems, key, &says);
        }
    }

    #[test]
    fn a_choice_field_whose_count_the_template_does_not_print_is_named() {
        names_file_and_key(
            &problems_of("{\"a\": \"{count:key|keys} left\"}"),
            "a",
            "the choice field {count:key|keys} chooses by {count}, but the template does not \
             print {count}",
        );
    }

    #[test]
    fn a_choice_field_is_a_choice_piece_at_the_position_of_its_count() {
        let templates = parse(FILE, "{\"a\": \"{path}: {count} {count:key|keys}\"}").unwrap();
        let template = &templates[0];
        assert_eq!(
            template.values,
            [("path", Kind::Text), ("count", Kind::Count)]
        );
        assert_eq!(
            template.pieces,
            [
                Piece::Value(0),
                Piece::Text(": ".to_owned()),
                Piece::Value(1),
                Piece::Text(" ".to_owned()),
                Piece::Choice(1, "key".to_owned(), "keys".to_owned()),
            ]
        );
        let text = module("The message templates.", &templates);
        assert!(
            text.contains("Piece::Choice(1, \"key\", \"keys\")"),
            "{text}"
        );
    }

    #[test]
    fn a_placeholder_that_the_table_lacks_is_named() {
        names_file_and_key(
            &problems_of("{\"a\": \"{nobody}\"}"),
            "a",
            "{nobody} is not in the type table",
        );
    }

    #[test]
    fn a_template_with_more_than_six_placeholders_is_named() {
        let text = "{\"a\": \"{path}{start}{end}{line}{count}{total}{note}\"}";
        names_file_and_key(&problems_of(text), "a", "has 7 placeholders");
    }

    #[test]
    fn values_follow_the_table_order_and_a_repeated_name_reuses_its_position() {
        let templates = parse(FILE, "{\"z\": \"{end} of {path} from {start} to {end}\"}").unwrap();
        let template = &templates[0];
        assert_eq!(
            template.values,
            [
                ("path", Kind::Text),
                ("start", Kind::Count),
                ("end", Kind::Count)
            ]
        );
        assert_eq!(
            template.pieces,
            [
                Piece::Value(2),
                Piece::Text(" of ".to_owned()),
                Piece::Value(0),
                Piece::Text(" from ".to_owned()),
                Piece::Value(1),
                Piece::Text(" to ".to_owned()),
                Piece::Value(2),
            ]
        );
    }

    #[test]
    fn the_module_declares_each_template_with_its_signature() {
        let text = concat!(
            "{\"b_one\": \"{count} left\", \"a_fixed\": \"\\\"x\\\"\", ",
            "\"c_two\": \"{note} {path}\"}"
        );
        let templates = parse(FILE, text).unwrap();
        let keys: Vec<&str> = templates
            .iter()
            .map(|template: &Template| template.key.as_str())
            .collect();
        assert_eq!(keys, ["a_fixed", "b_one", "c_two"]);
        let text = module("The message templates.", &templates);
        assert!(text.contains("use crate::template::{Count, Piece, Template, Text};"));
        assert!(text.contains("pub const A_FIXED: &str = \"\\\"x\\\"\";"));
        assert!(
            text.contains("/// A template that takes `count`.\npub const B_ONE: Template<Count> =")
        );
        assert!(text.contains(
            "/// A template that takes (`path`, `note`).\npub const C_TWO: Template<(Text, Text)> ="
        ));
        let catalog = catalog(&[("messages", &templates)]);
        assert!(
            catalog.contains("(\"messages\", \"a_fixed\", &[], &[Piece::Text(messages::A_FIXED)])")
        );
        assert!(catalog.contains(
            "(\"messages\", \"c_two\", &[\"path\", \"note\"], messages::C_TWO.pieces())"
        ));
    }
}
