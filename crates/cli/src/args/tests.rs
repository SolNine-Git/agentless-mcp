use std::fs;
use std::path::Path;

use serde_json::Value;

use super::declared::float_text;
use super::wrap::{collapse, wrap};
use super::{Command, Kind, Parser};
use crate::commands;

fn wrap_cases() -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/wrap.json");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let document: Value = serde_json::from_str(&text).expect("a JSON fixture");
    document["cases"].as_array().expect("a case list").clone()
}

fn number(value: &Value) -> usize {
    usize::try_from(value.as_u64().expect("a number")).expect("a size")
}

#[test]
fn each_text_wraps_to_the_fixture_lines_at_every_width() {
    let cases = wrap_cases();
    assert!(cases.len() > 30, "the fixture holds {} texts", cases.len());
    for case in &cases {
        let collapsed = collapse(case["text"].as_str().expect("a text"));
        assert_eq!(
            collapsed,
            case["collapsed"].as_str().expect("a collapsed text")
        );
        for run in case["runs"].as_array().expect("a run list") {
            let lines = run[2].as_array().expect("a line list");
            let want: Vec<&str> = lines
                .iter()
                .map(|slice| &collapsed[number(&slice[0])..number(&slice[0]) + number(&slice[1])])
                .collect();
            for width in number(&run[0])..=number(&run[1]) {
                assert_eq!(
                    wrap(&collapsed, width),
                    want,
                    "{collapsed:?} at width {width}"
                );
            }
        }
    }
}

fn float_cases(section: &str) -> Vec<(String, Option<String>)> {
    io::testing::cases(env!("CARGO_MANIFEST_DIR"), "read/numbers", section)
}

#[test]
fn a_float_text_reads_as_the_reference_reads_it() {
    let cases = float_cases("floats");
    assert!(cases.len() > 30);
    for (text, bits) in cases {
        let read = float_text(&text).map(|value| format!("0x{:016x}", value.to_bits()));
        assert_eq!(read, bits, "{text:?}");
    }
}

// Ledger deviation: the reference reads Unicode digits, and this build refuses them.
#[test]
fn a_unicode_digit_float_is_refused_where_the_reference_reads_it() {
    for (text, bits) in float_cases("deviations") {
        assert!(bits.is_some(), "{text:?}: the reference reads it");
        assert_eq!(float_text(&text), None, "{text:?}");
    }
}

// The wrapper reads bytes as ASCII characters, and the formatter prints help text as written.
#[test]
fn every_help_text_is_ascii_and_holds_no_format_mark() {
    let mut parsers = vec![commands::top()];
    let mut texts: Vec<String> = Vec::new();
    let mut at = 0;
    while let Some(parser) = parsers.get(at) {
        let commands: Vec<&Command> = (parser.actions.iter())
            .filter_map(|action| match action.kind {
                Kind::Commands(commands) => Some(commands),
                _ => None,
            })
            .flatten()
            .collect();
        texts.extend(commands.iter().map(|command| command.help.to_owned()));
        let nested: Vec<Parser> = commands.into_iter().map(Command::parser).collect();
        parsers.extend(nested);
        at += 1;
    }
    // The top parser, one per subcommand, and the nested ones.
    assert!(
        parsers.len() > 1 + commands::COMMANDS.len(),
        "{} parsers",
        parsers.len()
    );
    for parser in &parsers {
        texts.extend(parser.description.map(str::to_owned));
        texts.extend(
            parser
                .actions
                .iter()
                .filter_map(|action| action.help.clone()),
        );
    }
    for text in texts {
        assert!(text.is_ascii() && !text.contains('%'), "{text}");
    }
}
