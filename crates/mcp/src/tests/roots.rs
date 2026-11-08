use prompts::messages::REPO_REFUSED_NOT_ALLOWED;
use serde_json::json;

use super::{Sandbox, masked, probe};
use crate::RootsFile;
use crate::dispatch::{UNPLANNED, failed};
use crate::failure::Failure;
use crate::roots::{Allowlist, Anchor, Selection, unauthorised};

// The reference's refusal at a call after `./roots.txt` goes, or turns into a directory.
const RELATIVE_GONE: &str = "roots file roots.txt cannot be read: [Errno 2] No such file or \
     directory: 'roots.txt'. Refusing rather than serving the stale copy; restore the file, or \
     restart the server without --roots-from.";
const RELATIVE_DIRECTORY: &str = "roots file roots.txt cannot be read: [Errno 21] Is a directory: \
     'roots.txt'. Refusing rather than serving the stale copy; restore the file, or restart the \
     server without --roots-from.";
// The parts of the reference's roots refusals, which name no tool after the prefix.
const PREFIX: &str = "Error calling tool 'search': ";
const REQUIRED: &str = "repo_root is required: this server holds several repositories and will \
     not guess between them. Retry with repo_root set to one of: ";
const NOT_ALLOWED: &str = "repository refused: the requested root is not one of this server's \
     roots. Retry with repo_root set to one of: ";
const NO_ROOTS: &str = "no repositories are served: this server was started with no roots. Roots \
     come from --root DIR flags and from --roots-from FILE, which lists one repository path per \
     line. A root advertised by the client can only select among the roots the server is \
     configured with; it cannot add one.";
const HINT: &str = " To serve another repository, append its absolute path as a line in \
     <SANDBOX>/roots.txt; the file is re-read on the next call, no restart needed.";
const UNREADABLE: &str = ". Refusing rather than serving the stale copy; restore the file, or \
     restart the server without --roots-from.";

// Three repositories, `one` with `one/inner` below it, and `two`.
fn three_repositories() -> Sandbox {
    let sandbox = Sandbox::new();
    for repo in ["one", "two", "one/inner"] {
        sandbox.step(&["init", repo]);
        let function = "def a():\n    return 1\n";
        sandbox.step(&["write", &format!("{repo}/a.py"), function]);
        let caller = "from a import a\n\n\ndef c():\n    return a()\n";
        sandbox.step(&["write", &format!("{repo}/b/c.py"), caller]);
        sandbox.step(&["commit", repo]);
    }
    sandbox
}

// The masked answer of `server` to a file listing that names `repo_root`, and its error flag. A
// listing reads only the walk.
fn answered(sandbox: &Sandbox, server: &crate::Server, repo_root: Option<&str>) -> (String, bool) {
    let mut call = json!({"files": "*.py"});
    if let Some(root) = repo_root {
        call["repo_root"] = json!(sandbox.placed(root));
    }
    let answer = server.answer("search", call.as_object().cloned());
    (masked(&sandbox.normalised(&answer.text)), answer.is_error)
}

// What a roots case expects: a refusal after the tool's prefix, the unplanned-error answer, or an
// answer from this root.
enum Want<'a> {
    Refused(String),
    Unplanned,
    From(&'a str),
}

fn refused(parts: &[&str]) -> Want<'static> {
    Want::Refused(format!("{PREFIX}{}", parts.concat()))
}

fn assert_case(sandbox: &Sandbox, server: &crate::Server, repo_root: Option<&str>, want: &Want) {
    let (text, error) = answered(sandbox, server, repo_root);
    match want {
        Want::Refused(refusal) => {
            assert_eq!((&*text, error), (&**refusal, true), "{repo_root:?}");
        }
        Want::Unplanned => assert_eq!((&*text, error), (UNPLANNED, true), "{repo_root:?}"),
        Want::From(root) => {
            let receipt = format!("   repo: <SANDBOX>/{root}   ");
            let first = text.lines().next().unwrap_or_default();
            assert!(!error && first.contains(&receipt), "{repo_root:?}: {text}");
        }
    }
}

// The reference's answers to each root a call can name. Deviation D8: this build reads `~name` as
// a relative path, which no allowlist holds, where the reference fails on an unknown user.
#[test]
fn each_named_root_is_selected_or_refused_as_the_reference_says() {
    let sandbox = three_repositories();
    sandbox.step(&["write", "elsewhere/x.txt", "x\n"]);
    let others = [
        "<SANDBOX>/elsewhere",
        "relative/path",
        "~/x",
        "~nosuchuser_zz/x",
        "~root/x",
    ];
    for (roots, listed, inside, outside) in [
        (
            ["one", "two"],
            "<SANDBOX>/one, <SANDBOX>/two",
            "two",
            "<SANDBOX>/one/inner",
        ),
        (
            ["one", "one/inner"],
            "<SANDBOX>/one, <SANDBOX>/one/inner",
            "one/inner",
            "<SANDBOX>/two",
        ),
    ] {
        let server = sandbox.server(&roots, &[], None);
        assert_case(&sandbox, &server, None, &refused(&[REQUIRED, listed]));
        assert_case(&sandbox, &server, Some("<SANDBOX>/one"), &Want::From("one"));
        let inner = format!("<SANDBOX>/{inside}");
        assert_case(&sandbox, &server, Some(&inner), &Want::From(inside));
        let not_allowed = refused(&[NOT_ALLOWED, listed]);
        for root in std::iter::once(outside).chain(others) {
            assert_case(&sandbox, &server, Some(root), &not_allowed);
        }
        assert_case(&sandbox, &server, Some("a\u{0}b"), &Want::Unplanned);
    }
    let none = sandbox.server(&[], &[], None);
    for root in [None, Some("<SANDBOX>/one")] {
        assert_case(&sandbox, &none, root, &refused(&[NO_ROOTS]));
    }
}

// A roots file is read again at each call: its new lines select, its removed lines refuse, and a
// file that no longer reads refuses every call with its reason.
#[test]
fn a_roots_file_is_read_again_at_each_call() {
    let sandbox = three_repositories();
    sandbox.step(&["roots", "roots.txt", "# served", "", "  <SANDBOX>/two  "]);
    let server = sandbox.server(&["one"], &["roots.txt"], None);
    let two = "<SANDBOX>/one, <SANDBOX>/two";
    assert_case(&sandbox, &server, None, &refused(&[REQUIRED, two, HINT]));
    assert_case(&sandbox, &server, Some("<SANDBOX>/two"), &Want::From("two"));
    let inner = Some("<SANDBOX>/one/inner");
    assert_case(
        &sandbox,
        &server,
        inner,
        &refused(&[NOT_ALLOWED, two, HINT]),
    );
    sandbox.step(&[
        "roots",
        "roots.txt",
        "<SANDBOX>/two",
        "<SANDBOX>/one/inner",
        "one/inner/../inner",
    ]);
    assert_case(&sandbox, &server, inner, &Want::From("one/inner"));
    assert_case(&sandbox, &server, Some("<SANDBOX>/one"), &Want::From("one"));
    sandbox.step(&["roots", "roots.txt", "\u{feff}<SANDBOX>/one/inner"]);
    let nested = "<SANDBOX>/one, <SANDBOX>/one/inner";
    let two_refused = refused(&[NOT_ALLOWED, nested, HINT]);
    assert_case(&sandbox, &server, Some("<SANDBOX>/two"), &two_refused);
    assert_case(&sandbox, &server, inner, &Want::From("one/inner"));
    sandbox.step(&["bytes", "roots.txt", "\u{ff}\u{fe}"]);
    let undecoded = "roots file <SANDBOX>/roots.txt cannot be read: 'utf-8' codec can't decode \
                     byte 0xff in position 0: invalid start byte";
    let one = Some("<SANDBOX>/one");
    assert_case(&sandbox, &server, one, &refused(&[undecoded, UNREADABLE]));
    sandbox.step(&["remove", "roots.txt"]);
    let gone = "roots file <SANDBOX>/roots.txt cannot be read: [Errno 2] No such file or \
                directory: '<SANDBOX>/roots.txt'";
    for root in [one, None] {
        assert_case(&sandbox, &server, root, &refused(&[gone, UNREADABLE]));
    }
}

// Deviation QC2: with several roots and no repo_root, a stdio server answers from the one root
// that holds its launch directory, as it answers a call that names that root.
#[test]
fn several_roots_select_the_one_that_holds_the_launch_directory() {
    let sandbox = three_repositories();
    let server = sandbox.server(&["one", "two"], &[], Some("two/b"));
    let named = answered(&sandbox, &server, Some("<SANDBOX>/two"));
    assert_eq!(answered(&sandbox, &server, None), named);
    assert_case(&sandbox, &server, None, &Want::From("two"));
}

// A launch directory in none of the roots, or in two nested roots, selects nothing, and the call
// is refused with the listing, as a server with no launch directory refuses it.
#[test]
fn a_launch_directory_in_no_root_or_in_two_selects_nothing() {
    let sandbox = three_repositories();
    sandbox.step(&["write", "elsewhere/x.txt", "x\n"]);
    for (roots, launch, listed) in [
        (["one", "two"], "elsewhere", "<SANDBOX>/one, <SANDBOX>/two"),
        (
            ["one", "one/inner"],
            "one/inner/b",
            "<SANDBOX>/one, <SANDBOX>/one/inner",
        ),
    ] {
        let server = sandbox.server(&roots, &[], Some(launch));
        assert_case(&sandbox, &server, None, &refused(&[REQUIRED, listed]));
    }
}

// A root swapped for a symlink after start-up resolves outside the allowlist, so a call with no
// repo_root gets the reference's refusal of a root that is not allowed (QC2 for the launch rule).
#[test]
fn a_root_swapped_for_a_symlink_is_refused() {
    for (roots, launch, listed) in [
        (&["one"][..], "one", "<SANDBOX>/one"),
        (&["one", "two"][..], "two/b", "<SANDBOX>/one, <SANDBOX>/two"),
    ] {
        let sandbox = three_repositories();
        sandbox.step(&["write", "elsewhere/x.txt", "x\n"]);
        let server = sandbox.server(roots, &[], Some(launch));
        assert!(!answered(&sandbox, &server, None).1, "{roots:?}");
        let swapped = roots[roots.len() - 1];
        io::remove_tree(&sandbox.root.join(swapped)).expect("a removed root");
        let target = sandbox.root.join("elsewhere");
        sandbox.tree.symlink(swapped, target).expect("a symlink");
        let want = refused(&[NOT_ALLOWED, listed]);
        assert_case(&sandbox, &server, None, &want);
    }
}

// tools/list publishes repo_root only when a call can select a root: two distinct roots, or a
// roots file, whose lines can change after start-up.
#[test]
fn the_tool_list_publishes_repo_root_only_when_a_call_can_select_a_root() {
    let sandbox = three_repositories();
    sandbox.step(&["roots", "roots.txt", "<SANDBOX>/two"]);
    for (roots, files, published) in [
        (&[][..], &[][..], false),
        (&["one"][..], &[][..], false),
        (&["one", "one"][..], &[][..], false),
        (&["one", "two"][..], &[][..], true),
        (&[][..], &["roots.txt"][..], true),
        (&["one"][..], &["roots.txt"][..], true),
    ] {
        let server = sandbox.server(roots, files, None);
        assert_eq!(server.tools.len(), 2, "{roots:?} {files:?}");
        for tool in server.tools.iter() {
            let properties = tool.input_schema.get("properties");
            let listed = properties.and_then(|properties| properties.get("repo_root"));
            assert_eq!(
                listed.is_some(),
                published,
                "{roots:?} {files:?} {}",
                tool.name
            );
        }
    }
}

// One root needs no launch directory, and the launch directory never widens the allowlist.
#[test]
fn one_root_is_selected_wherever_the_server_starts() {
    let sandbox = probe();
    sandbox.step(&["write", "elsewhere/x.txt", "x\n"]);
    let anywhere = sandbox.server(&["repo"], &[], Some("elsewhere"));
    let nowhere = sandbox.server(&["repo"], &[], None);
    let answer = answered(&sandbox, &anywhere, None);
    assert!(!answer.1, "{}", answer.0);
    assert_eq!(answer, answered(&sandbox, &nowhere, None));
    let listing = refused(&[NOT_ALLOWED, "<SANDBOX>/repo"]);
    assert_case(&sandbox, &anywhere, Some("/tmp"), &listing);
}

// A roots file given as `./roots.txt` is named by its normalised path when a call finds it gone
// (the stat fails) or turned into a directory (the read fails).
#[test]
fn a_relative_roots_file_is_refused_by_its_normalised_path() {
    let sandbox = three_repositories();
    sandbox.step(&["roots", "roots.txt", "<SANDBOX>/two"]);
    let services = sandbox.services();
    let file = RootsFile::read("./roots.txt", &services).expect("a roots file");
    let allowlist = Allowlist::new(Vec::new(), vec![file]);
    sandbox.step(&["remove", "roots.txt"]);
    let refused = allowlist.current().expect_err("a refusal");
    assert_eq!(refused, Failure::Refused(RELATIVE_GONE.to_owned()));
    sandbox.step(&["write", "roots.txt/x", "x\n"]);
    let refused = allowlist.current().expect_err("a refusal");
    assert_eq!(refused, Failure::Refused(RELATIVE_DIRECTORY.to_owned()));
}

// A root swapped after its check gets the reference's not-allowed refusal with its selection's
// hint: the roots-file hint for a root that the call names, none for the only root.
#[test]
fn a_swapped_root_is_refused_with_the_hint_of_its_selection() {
    let sandbox = three_repositories();
    sandbox.step(&["roots", "roots.txt", "<SANDBOX>/two"]);
    let server = sandbox.server(&["one"], &["roots.txt"], None);
    let inner = &*server.inner;
    let anchor = Anchor::of(&inner.services);
    let hint = inner.allowlist.hint();
    let allowed = inner.allowlist.current().expect("the roots");
    let requested = sandbox.placed("<SANDBOX>/one/inner");
    let named = Selection {
        allowed: &allowed,
        requested: Some(&requested),
        launch: None,
        anchor: &anchor,
        hint: hint.as_deref(),
    };
    let text = failed(inner, "search", unauthorised(&named)).text;
    let want = format!("{PREFIX}{NOT_ALLOWED}<SANDBOX>/one, <SANDBOX>/two{HINT}");
    assert_eq!(masked(&sandbox.normalised(&text)), want);
    sandbox.step(&["roots", "roots.txt", "<SANDBOX>/one"]);
    let only = inner.allowlist.current().expect("the roots");
    assert_eq!(only.len(), 1, "{only:?}");
    let sole = Selection {
        allowed: &only,
        requested: None,
        ..named
    };
    let listing = only[0].display().to_string();
    let want = Failure::Refused(REPO_REFUSED_NOT_ALLOWED.render(&listing));
    assert_eq!(unauthorised(&sole), want);
}

// A call into a root that is not a git repository answers, and its receipt says what is unknown.
#[test]
fn a_root_outside_git_answers_with_its_note() {
    let sandbox = Sandbox::new();
    sandbox.step(&["write", "plain/a.py", "def a():\n    return 1\n"]);
    let server = sandbox.server(&["plain"], &[], Some("plain"));
    let (text, error) = answered(&sandbox, &server, None);
    let note = "// note: <SANDBOX>/plain is not inside a git repository: HEAD and dirty count \
                are unknown";
    assert!(!error && text.lines().nth(1) == Some(note), "{text}");
}
