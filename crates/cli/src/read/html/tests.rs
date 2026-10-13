use std::ffi::OsStr;
use std::io::Error;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use super::{cache_name, write_failure};
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

#[test]
fn a_write_failure_names_the_errno_and_the_target() {
    let denied = Error::from_raw_os_error(13);
    assert_eq!(
        write_failure(&denied, Path::new("/c/exports/g.html")),
        "[Errno 13] Permission denied: '/c/exports/g.html'"
    );
    let plain = Error::other("no errno");
    assert_eq!(write_failure(&plain, Path::new("/c/g.html")), "no errno");
}
