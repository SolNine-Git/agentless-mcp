use kernel::Key;
use sha2::{Digest, Sha256};

use super::indexed::file;
use crate::blob::decode_facts;
use crate::{FACTS_VERSION, analyze, encode};

// Each blob shape: decorated definitions, imports, references, and the metrics of a file that
// parses and the stored reason of one that does not.
const TEXTS: [(&str, &str); 4] = [
    (
        "a.py",
        "import os\nfrom . import b as c\n\n\n@cache\ndef size(x):\n    return os.sep + c.d(x)\n",
    ),
    ("b.py", "def broken():\n    return )\n"),
    (
        "c.js",
        "import { join } from 'path';\nexport class Box {\n  size(x) {\n    return join(x);\n  }\n}\n",
    ),
    (
        "d.rs",
        "use std::fmt;\n\n#[derive(Debug)]\npub struct Box;\n\nimpl Box {\n    fn size(&self) -> String {\n        fmt::format(format_args!(\"{self:?}\"))\n    }\n}\n",
    ),
];

// A stored row is served while FACTS_VERSION holds, so these blobs change only with a bump.
const PINNED: (u32, &str) = (
    13,
    "b075c424ddf4c5c8109841e8daaaeae0d67d597f962d89ee4fde694b7e06a6fe",
);

fn blob_digest() -> String {
    let mut hasher = Sha256::new();
    for (path, text) in TEXTS {
        let analysis = analyze(&file(path, text), true).expect("the text parses");
        let blobs = encode(&analysis).blobs.expect("the blobs encode");
        let metrics = blobs.metrics.expect("a measured text has a metrics blob");
        for blob in [blobs.symbols, blobs.rest, metrics] {
            let length = u64::try_from(blob.len()).expect("a small blob");
            hasher.update(length.to_le_bytes());
            hasher.update(&blob);
        }
    }
    Key::new(hasher.finalize().into()).to_string()
}

#[test]
fn the_blobs_of_the_sample_texts_change_only_with_a_facts_version_bump() {
    let current = (FACTS_VERSION, blob_digest());
    assert_eq!(
        (current.0, current.1.as_str()),
        PINNED,
        "the blobs or FACTS_VERSION changed: bump FACTS_VERSION for a blob change, then pin the \
         new version with the new digest"
    );
}

#[test]
fn a_manifest_fact_reads_back_from_the_rest_blob_and_a_file_without_one_stores_none() {
    let texts = [
        (
            "Cargo.toml",
            "[package]\nname = \"a-b\"\n\n[lib]\npath = \"lib.rs\"\n",
        ),
        ("d.rs", TEXTS[3].1),
    ];
    for (path, text) in texts {
        let analysis = analyze(&file(path, text), false).expect("the text parses");
        let blobs = encode(&analysis).blobs.expect("the blobs encode");
        let read = decode_facts(&blobs.symbols, &blobs.rest).expect("the blobs decode");
        assert_eq!(read, analysis.facts, "{path}");
        let stored = String::from_utf8(blobs.rest).expect("a JSON blob");
        assert_eq!(
            stored.contains("\"manifest\""),
            path == "Cargo.toml",
            "{stored}"
        );
    }
}

#[test]
fn the_mod_items_read_back_from_the_rest_blob_and_a_file_without_one_stores_none() {
    let texts = [
        (
            "m.rs",
            "mod inner {}\n#[path = \"x.rs\"]\nmod moved;\n",
            true,
        ),
        ("d.rs", TEXTS[3].1, false),
    ];
    for (path, text, held) in texts {
        let analysis = analyze(&file(path, text), false).expect("the text parses");
        let blobs = encode(&analysis).blobs.expect("the blobs encode");
        let read = decode_facts(&blobs.symbols, &blobs.rest).expect("the blobs decode");
        assert_eq!(read, analysis.facts, "{path}");
        assert_eq!(read.modules.len(), if held { 2 } else { 0 }, "{path}");
        let stored = String::from_utf8(blobs.rest).expect("a JSON blob");
        assert_eq!(stored.contains("\"modules\""), held, "{stored}");
    }
}
