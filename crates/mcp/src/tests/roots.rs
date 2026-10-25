use prompts::messages::REPO_REFUSED_NOT_ALLOWED;
use serde_json::json;

use super::calls::{answered, built, masked, reference, run_named};
use crate::RootsFile;
use crate::dispatch::failed;
use crate::failure::Failure;
use crate::roots::{Allowlist, Anchor, Selection, unauthorised};

// The reference's refusal at a call after `./roots.txt` goes, or turns into a directory.
const RELATIVE_GONE: &str = "roots file roots.txt cannot be read: [Errno 2] No such file or \
     directory: 'roots.txt'. Refusing rather than serving the stale copy; restore the file, or \
     restart the server without --roots-from.";
const RELATIVE_DIRECTORY: &str = "roots file roots.txt cannot be read: [Errno 21] Is a directory: \
     'roots.txt'. Refusing rather than serving the stale copy; restore the file, or restart the \
     server without --roots-from.";

// Deviation QC2: with several roots and no repo_root, a stdio server answers from the one root
// that holds its launch directory, as the reference answers a call that names that root.
#[test]
fn several_roots_select_the_one_that_holds_the_launch_directory() {
    let run = run_named("several_roots");
    let sandbox = built(&run);
    let server = sandbox.server(&["one", "two"], &[], Some("two/b"));
    let named = json!({"operation": "map", "repo_root": "<SANDBOX>/two"});
    let want = (reference(&run, "orient", &named), false);
    assert_eq!(
        answered(&sandbox, &server, "orient", &json!({"operation": "map"})),
        want
    );
    let capabilities = json!({"repo_root": "<SANDBOX>/two"});
    let want = (reference(&run, "capabilities", &capabilities), false);
    assert_eq!(
        answered(&sandbox, &server, "capabilities", &json!({})),
        want
    );
}

// A launch directory in none of the roots, or in two nested roots, selects nothing, and the call
// is refused with the listing, as a server with no launch directory refuses it.
#[test]
fn a_launch_directory_in_no_root_or_in_two_selects_nothing() {
    for (name, roots, launch) in [
        ("several_roots", ["one", "two"], "elsewhere"),
        ("nested_roots", ["one", "one/inner"], "one/inner/b"),
    ] {
        let run = run_named(name);
        let sandbox = built(&run);
        sandbox.step(&["write", "elsewhere/x.txt", "x\n"]);
        let server = sandbox.server(&roots, &[], Some(launch));
        let call = json!({"operation": "map"});
        let want = (reference(&run, "orient", &call), true);
        assert_eq!(answered(&sandbox, &server, "orient", &call), want, "{name}");
    }
}

// A root swapped for a symlink after start-up resolves outside the allowlist, so a call with no
// repo_root gets the reference's refusal of a root that is not allowed (QC2 for the launch rule).
#[test]
fn a_root_swapped_for_a_symlink_is_refused() {
    for (name, roots, launch, tool, named) in [
        (
            "probe",
            &["repo"][..],
            "repo",
            "graph",
            json!({"operation": "cycles", "repo_root": "/tmp"}),
        ),
        (
            "several_roots",
            &["one", "two"][..],
            "two/b",
            "orient",
            json!({"operation": "map", "repo_root": "<SANDBOX>/elsewhere"}),
        ),
    ] {
        let run = run_named(name);
        let sandbox = built(&run);
        sandbox.step(&["write", "elsewhere/x.txt", "x\n"]);
        let server = sandbox.server(roots, &[], Some(launch));
        let call = json!({"operation": named["operation"].clone()});
        assert!(!answered(&sandbox, &server, tool, &call).1, "{name}");
        let swapped = roots[roots.len() - 1];
        io::remove_tree(&sandbox.root.join(swapped)).expect("a removed root");
        let target = sandbox.root.join("elsewhere");
        sandbox.tree.symlink(swapped, target).expect("a symlink");
        let want = (reference(&run, tool, &named), true);
        assert_eq!(answered(&sandbox, &server, tool, &call), want, "{name}");
    }
}

// tools/list publishes repo_root only when a call can select a root: two distinct roots, or a
// roots file, whose lines can change after start-up.
#[test]
fn the_tool_list_publishes_repo_root_only_when_a_call_can_select_a_root() {
    let run = run_named("roots_file");
    let sandbox = built(&run);
    for (roots, files, published) in [
        (&[][..], &[][..], false),
        (&["one"][..], &[][..], false),
        (&["one", "one"][..], &[][..], false),
        (&["one", "two"][..], &[][..], true),
        (&[][..], &["roots.txt"][..], true),
        (&["one"][..], &["roots.txt"][..], true),
    ] {
        let server = sandbox.server(roots, files, None);
        assert_eq!(server.tools.len(), 7, "{roots:?} {files:?}");
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
    let run = run_named("probe");
    let sandbox = built(&run);
    sandbox.step(&["write", "elsewhere/x.txt", "x\n"]);
    let server = sandbox.server(&["repo"], &[], Some("elsewhere"));
    let call = json!({"operation": "map"});
    let want = (reference(&run, "orient", &call), false);
    assert_eq!(answered(&sandbox, &server, "orient", &call), want);
}

// A roots file given as `./roots.txt` is named by its normalised path when a call finds it gone
// (the stat fails) or turned into a directory (the read fails).
#[test]
fn a_relative_roots_file_is_refused_by_its_normalised_path() {
    let run = run_named("roots_file");
    let sandbox = built(&run);
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
    let run = run_named("roots_file");
    let sandbox = built(&run);
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
    let refused = failed(inner, "orient", unauthorised(&named)).text;
    let call = json!({"operation": "map", "repo_root": "<SANDBOX>/one/inner"});
    let want = reference(&run, "orient", &call);
    assert_eq!(masked(&sandbox.normalised(&refused)), want);
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
