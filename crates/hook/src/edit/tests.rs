use io::testing::cases;
use text::CodePointString;

use super::{Eol, LoneSurrogate, Proposal, proposal};
use crate::halt::Halt;
use crate::payload::{Change, EditInput};
use crate::record::Outcome;
use crate::testing::MANIFEST_DIR;

// One generated case: the tool, the old text (None: no file), the content or old_string, the
// new_string, replace_all, and the expected ["text", text, eol] or ["skip", reason].
type EditCase = (
    String,
    Option<String>,
    String,
    Option<String>,
    bool,
    serde_json::Value,
);

fn change(kind: &str, first: &str, second: Option<&str>, replace_all: bool) -> Change {
    if kind == "write" {
        return Change::Write(CodePointString::from(first));
    }
    Change::Edit(EditInput {
        old_string: CodePointString::from(first),
        new_string: CodePointString::from(second.expect("an Edit case has a new_string")),
        replace_all,
    })
}

fn shown(result: Result<Proposal, Halt>) -> serde_json::Value {
    match result {
        Ok(Proposal {
            text: Ok(text),
            eol,
        }) => {
            let eol = eol.map(|eol| serde_json::to_value(eol).expect("an eol name"));
            serde_json::json!(["text", text, eol])
        }
        Ok(Proposal { text: Err(_), .. }) => serde_json::json!(["surrogate"]),
        Err(halt) => {
            assert_eq!(halt.outcome, Outcome::Skipped, "{halt:?}");
            serde_json::json!(["skip", halt.reason])
        }
    }
}

#[test]
fn the_proposed_text_matches_the_generated_cases() {
    let all: Vec<EditCase> = cases(MANIFEST_DIR, "edit", "edit");
    assert!(all.len() > 1000, "{} cases", all.len());
    for (kind, old, first, second, replace_all, expected) in all {
        let change = change(&kind, &first, second.as_deref(), replace_all);
        let found = shown(proposal(&change, old.as_deref()));
        assert_eq!(
            found, expected,
            "{kind} old {old:?} {first:?} {second:?} {replace_all}"
        );
    }
}

#[test]
fn a_lone_surrogate_in_new_string_leaves_no_utf8_text_but_keeps_the_eol() {
    let edit = Change::Edit(EditInput {
        old_string: CodePointString::from("b"),
        new_string: CodePointString::from_utf8_escaped(b"\xff"),
        replace_all: false,
    });
    let found = proposal(&edit, Some("a\r\nb\r\n")).expect("the Edit applies");
    assert_eq!(found.text, Err(LoneSurrogate));
    assert_eq!(found.eol, Some(Eol::Crlf));
}

#[test]
fn a_lone_surrogate_in_old_string_is_never_in_the_file() {
    let edit = Change::Edit(EditInput {
        old_string: CodePointString::from_utf8_escaped(b"\xff"),
        new_string: CodePointString::from("x"),
        replace_all: false,
    });
    let halt = proposal(&edit, Some("a\n")).expect_err("Claude Code fails this Edit");
    assert_eq!(
        halt.reason,
        "old_string is not in the file, so Claude Code fails this edit"
    );
}
