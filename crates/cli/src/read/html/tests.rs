use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

use super::cache_name;
use crate::args::Arg;

fn accepted(raw: &[u8]) -> bool {
    cache_name(&Arg::new(OsStr::from_bytes(raw))).is_some()
}

#[test]
fn a_cache_name_is_one_simple_html_file_name() {
    let good: [&[u8]; 4] = [b"graph.html", b"a.html", b"v1.2_x-y.html", b"x.html.html"];
    assert!(good.iter().all(|name| accepted(name)));
    let bad: [&[u8]; 8] = [
        b".html",
        b"_a.html",
        b"a/b.html",
        b"../a.html",
        b"a.HTML",
        b"a.htm",
        "café.html".as_bytes(),
        b"x\xffy.html",
    ];
    assert!(bad.iter().all(|name| !accepted(name)));
    let longest = format!("{}.html", "a".repeat(123));
    assert!(accepted(longest.as_bytes()));
    assert!(!accepted(format!("a{longest}").as_bytes()));
}
