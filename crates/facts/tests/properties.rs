//! Properties over seeded random inputs: stable ids read back what they display, a reorder of two
//! functions keeps the ordinals of the others, and every signature and import keeps its shape.

use std::fmt::Write as _;
use std::str::FromStr;
use std::sync::Arc;

use facts::{StableId, extract};
use serde::Deserialize;
use syntax::{Language, parse};
use text::testing::cases;

// xorshift64*, seeded, so that each run draws the same inputs.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(bound).expect("a small bound"))
            .expect("a value below the bound")
    }

    fn text(&mut self, alphabet: &[&str], longest: usize) -> String {
        let length = self.below(longest + 1);
        (0..length)
            .map(|_| alphabet[self.below(alphabet.len())])
            .collect()
    }
}

// Mostly name characters, with each separator, blank and control character the parser weighs.
const ID_ALPHABET: [&str; 20] = [
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "/",
    ".",
    ":",
    "::",
    "#",
    " ",
    "\t",
    "\n",
    "\u{3000}",
    "\u{e9}",
    "\u{7f}",
    "rationale@",
    "_",
];

#[test]
fn an_accepted_id_text_displays_back_to_itself() {
    let mut random = Random(0x9e37_79b9_7f4a_7c15);
    let mut accepted = 0;
    for _ in 0..20_000 {
        let text = format!(
            "{}:{}::{}",
            random.text(&ID_ALPHABET, 4),
            random.text(&ID_ALPHABET, 6),
            random.text(&ID_ALPHABET, 6)
        );
        if let Ok(id) = StableId::from_str(&text) {
            assert_eq!(id.to_string(), text);
            accepted += 1;
        }
    }
    assert!(accepted > 1000, "{accepted} of 20000 texts are ids");
}

// The precondition: a prefix with no `:`, a path with no `::` that does not end in `:`, parts
// that are not blank and hold no control character, and no rationale marker in the text.
#[test]
fn parts_that_meet_the_precondition_read_back_from_their_id() {
    let mut random = Random(0x0123_4567_89ab_cdef);
    let mut checked = 0;
    for _ in 0..20_000 {
        let parts = [
            random.text(&ID_ALPHABET, 4),
            random.text(&ID_ALPHABET, 6),
            random.text(&ID_ALPHABET, 6),
        ];
        let id = StableId::new(&parts[0], &parts[1], &parts[2]);
        if !precondition(&parts, &id.to_string()) {
            continue;
        }
        let read = StableId::from_str(&id.to_string()).expect("a well-formed id parses");
        assert_eq!(read, id, "{parts:?}");
        checked += 1;
    }
    assert!(
        checked > 1000,
        "{checked} of 20000 part sets meet the precondition"
    );
}

fn precondition(parts: &[String; 3], text: &str) -> bool {
    let well_formed = parts.iter().all(|part| {
        !text::strip(part).is_empty() && !part.chars().any(|c| c < ' ' || c == '\u{7f}')
    });
    well_formed
        && !parts[0].contains(':')
        && !parts[1].contains("::")
        && !parts[1].ends_with(':')
        && !text.contains("::rationale@")
}

const NAMES: [&str; 3] = ["f", "g", "h"];

// A reorder of two top-level functions keeps the ordinal of each other function whose name
// neither moved function shares.
#[test]
fn a_reorder_of_two_functions_keeps_the_other_ordinals() {
    let mut random = Random(0xfeed_beef_dead_cafe);
    let mut compared = 0;
    for _ in 0..300 {
        let names: Vec<&str> = (0..2 + random.below(6))
            .map(|_| NAMES[random.below(NAMES.len())])
            .collect();
        let (first, second) = (random.below(names.len()), random.below(names.len()));
        let mut moved = names.clone();
        moved.swap(first, second);
        let before = ordinals(&names, &mut random);
        let after = ordinals(&moved, &mut random);
        for (position, name) in names.iter().enumerate() {
            if position == first || position == second {
                continue;
            }
            if *name == names[first] || *name == names[second] {
                continue;
            }
            assert_eq!(
                after[position], before[position],
                "{names:?} swap {first} {second}"
            );
            compared += 1;
        }
    }
    assert!(compared > 100, "{compared} ordinals compared");
}

// The duplicate index of each function of a module that defines `names` in order, each body one
// to three lines long.
fn ordinals(names: &[&str], random: &mut Random) -> Vec<u32> {
    let mut source = String::new();
    for name in names {
        let body = "    x = 1\n".repeat(1 + random.below(3));
        write!(source, "def {name}():\n{body}\n").expect("a String takes every write");
    }
    let parsed = parse(Language::Python, Arc::from(source.as_str())).expect("it parses");
    let symbols = extract(&parsed).symbols;
    assert_eq!(symbols.len(), names.len(), "{source}");
    symbols
        .iter()
        .map(|symbol| symbol.duplicate_index)
        .collect()
}

#[derive(Deserialize)]
struct Source {
    name: String,
    source: String,
}

// Every signature is one line of at most 80 code points with no space at either end, and every
// import names its local names by position or not at all.
#[test]
fn every_signature_and_import_keeps_its_shape() {
    let mut symbols = 0;
    for language in Language::ALL {
        let file = format!("extract_{}", language.name());
        let all: Vec<Source> = cases(env!("CARGO_MANIFEST_DIR"), &file, "cases");
        for source in all {
            let parsed = parse(language, Arc::from(source.source.as_str())).expect("it parses");
            let facts = extract(&parsed);
            for symbol in &facts.symbols {
                let signature = &symbol.signature;
                assert!(!signature.contains('\n'), "{}: {signature:?}", source.name);
                assert!(
                    signature.chars().count() <= 80,
                    "{}: {signature:?}",
                    source.name
                );
                assert_eq!(text::strip(signature), signature, "{}", source.name);
                symbols += 1;
            }
            for import in &facts.imports {
                let paired =
                    import.local_names.is_empty() || import.local_names.len() == import.names.len();
                assert!(paired, "{}: {import:?}", source.name);
            }
        }
    }
    assert!(symbols > 1000, "{symbols} symbols");
}
