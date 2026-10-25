use kernel::Key;
use sha2::{Digest, Sha256};

use super::indexed::file;
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
    5,
    "6b4022b5e3e83aadc8c421a4b5f5ee60b1ed091612eeb4a346904ccd41e0d22d",
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
