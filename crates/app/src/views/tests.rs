use super::{SkeletonRequest, SliceRequest, load, skeleton, slice, widen};
use crate::endpoints::scanned::Scanned;
use crate::testing::{MANIFEST_DIR, Sandbox};
use crate::{Error, IntArg};

#[test]
fn the_overview_of_a_data_file_is_its_key_outline_in_line_order() {
    let toml = "[workspace.package]\nversion = \"0.11.0\"\n[deps]\na = { b = 1 }\n";
    let files = [
        ("Cargo.toml", toml),
        ("rows.json", "[{\"id\": 1}, {\"id\": 2}]\n"),
    ];
    let scanned = Scanned::new(&files);
    let request = SkeletonRequest {
        paths: vec!["Cargo.toml".to_owned(), "rows.json".to_owned()],
        docstrings: None,
        numbered: false,
    };
    let views = skeleton(&scanned.reading(), &request);
    let unlisted = "more keys are not listed: the outline stops at depth 2, at list items and at \
        500 keys. Grep finds them.\n";
    let cargo = "table workspace.package, 1 key  @1\n  version = \"0.11.0\"  @2\n\
        table deps, 1 key  @3\n  a = object, 1 key  @4\n1 ";
    assert_eq!(views[0].text, format!("{cargo}{unlisted}"));
    assert_eq!(
        views[1].text,
        format!("document = list, 2 items\n2 {unlisted}")
    );
}

// The requested ranges, the context, and the reference view text.
type SliceCase = (Vec<(usize, usize)>, usize, String);

#[test]
fn each_slice_request_reports_or_renders_as_the_reference_does() {
    let files: Vec<(String, String)> = io::testing::cases(MANIFEST_DIR, "read/views", "slice_file");
    let (path, text) = &files[0];
    let scanned = Scanned::new(&[(path.as_str(), text.as_str())]);
    let all: Vec<SliceCase> = io::testing::cases(MANIFEST_DIR, "read/views", "slice");
    assert!(!all.is_empty());
    for (ranges, context, expected) in all {
        let request = SliceRequest {
            path: path.clone(),
            ranges: ranges.clone(),
            context: IntArg::from_usize(context),
        };
        let view = slice(&scanned.reading(), &request).value;
        let view = view.unwrap_or_else(|error| panic!("{ranges:?}: {error}"));
        assert_eq!(view.text, expected, "{ranges:?} at context {context}");
    }
}

#[test]
fn a_range_widens_by_the_context_within_the_file() {
    assert_eq!(widen(3, 5, 10, 134), (1, 15));
    assert_eq!(widen(130, 140, 2, 134), (128, 134));
    assert_eq!(widen(7, 7, 0, 134), (7, 7));
    assert_eq!(
        widen(usize::MAX, usize::MAX, 200, usize::MAX),
        (u32::MAX, u32::MAX)
    );
}

#[test]
fn the_root_itself_loads_as_dot_and_reads_as_a_reason() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("a.py", "x = 1\n")]);
    let loaded = load(&root, ".").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(loaded.path, ".");
    assert_eq!(loaded.language, None);
    let reason = loaded.text.expect_err("a directory has no text");
    assert!(reason.starts_with(".: unreadable: "), "{reason}");
}

#[test]
fn a_path_outside_the_root_is_a_security_refusal() {
    let sandbox = Sandbox::new();
    sandbox.put("outside.py", "x = 1\n");
    let root = sandbox.repo("repo", &[("a.py", "x = 1\n")]);
    let refused = load(&root, "../outside.py").err();
    assert!(matches!(
        refused,
        Some(Error::Kind(kernel::Error::SecurityRefusal(_)))
    ));
}

#[test]
fn a_file_in_no_language_keeps_its_text_with_replacements() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("a.py", "x = 1\n")]);
    sandbox.put("repo/notes.txt", b"x\xe9y\n");
    let loaded = load(&root, "notes.txt").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(loaded.text.as_deref().ok(), Some("x\u{fffd}y\n"));
}
