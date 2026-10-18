use serde_json::json;

use super::calls::{answered, built, reference, run_named};

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
    for (name, roots, launch, named) in [
        (
            "probe",
            &["repo"][..],
            "repo",
            json!({"operation": "cycles", "repo_root": "/tmp"}),
        ),
        (
            "several_roots",
            &["one", "two"][..],
            "two/b",
            json!({"operation": "map", "repo_root": "<SANDBOX>/elsewhere"}),
        ),
    ] {
        let run = run_named(name);
        let sandbox = built(&run);
        sandbox.step(&["write", "elsewhere/x.txt", "x\n"]);
        let server = sandbox.server(roots, &[], Some(launch));
        let call = json!({"operation": "map"});
        assert!(!answered(&sandbox, &server, "orient", &call).1, "{name}");
        let swapped = roots[roots.len() - 1];
        io::remove_tree(&sandbox.root.join(swapped)).expect("a removed root");
        let target = sandbox.root.join("elsewhere");
        sandbox.tree.symlink(swapped, target).expect("a symlink");
        let want = (reference(&run, "orient", &named), true);
        assert_eq!(answered(&sandbox, &server, "orient", &call), want, "{name}");
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
