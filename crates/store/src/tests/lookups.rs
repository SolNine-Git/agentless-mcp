use std::path::Path;

use serde_json::{Value, json};
use syntax::Language;

use super::cases::{BUSY, ROOT, Scratch, cases, source_text, usable};
use crate::Lookup;
use crate::open::open;

#[test]
fn each_lookup_gives_the_reference_blocks_reason_miss_or_warning() {
    for case in cases().lookups {
        let scratch = Scratch::new();
        scratch.damage(&case.damage, &case.path);
        let found = usable(open(&scratch.database, ROOT, BUSY), &case.name);
        let language = Language::from_name(&case.language).expect("a language name");
        let text = source_text(&case.path, &case.edit);
        let lookup = found.blocks_for(&case.path, &text, language);
        let outcome = shown(&lookup, &scratch.database);
        assert_eq!(outcome, case.outcome, "{}", case.name);
    }
}

#[test]
fn a_key_or_a_line_that_a_block_cannot_hold_refuses_the_rows() {
    let damages = [
        (
            "UPDATE blocks SET type2_key = 'zz' WHERE file_id = :file_id",
            "ValueError(\"type2_key 'zz' is not 64 lowercase hexadecimal characters\")",
        ),
        (
            "UPDATE blocks SET end_line = -1 WHERE file_id = :file_id",
            "ValueError('end_line -1 is not between 0 and 4294967295')",
        ),
    ];
    let path = "complexity_py/clones.py";
    for (statement, cause) in damages {
        let scratch = Scratch::new();
        scratch.damage(&[json!("sql"), json!([statement])], path);
        let found = usable(open(&scratch.database, ROOT, BUSY), statement);
        let lookup = found.blocks_for(path, &source_text(path, "none"), Language::Python);
        let warning = format!(
            "tag cache {{db}}: block rows for {path} are unreadable ({cause}); parsing the file \
             instead"
        );
        let outcome = shown(&lookup, &scratch.database);
        assert_eq!(outcome, json!(["unreadable", warning]));
    }
}

// The lookup in the form that the fixtures record, with the database path written {db}.
fn shown(lookup: &Lookup, database: &Path) -> Value {
    match lookup {
        Lookup::Hit(blocks) => {
            let rows: Vec<Value> = blocks
                .iter()
                .map(|block| {
                    json!([
                        block.start_line,
                        block.end_line,
                        block.key.to_string(),
                        block.exact.to_string(),
                        block.function,
                        block.tokens
                    ])
                })
                .collect();
            json!(["hit", rows])
        }
        Lookup::Unmeasured(reason) => json!(["unmeasured", reason]),
        Lookup::Miss => json!(["miss"]),
        Lookup::Unreadable(failure) => {
            let text = failure.to_string();
            let placed = text.replace(&database.display().to_string(), "{db}");
            json!(["unreadable", placed])
        }
    }
}
