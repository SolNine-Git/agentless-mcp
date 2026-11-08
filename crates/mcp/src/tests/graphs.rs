//! The graph that a server keeps for a root: built once while the scan is the same, built again
//! after a file changes, and answering as a fresh server does.

use std::sync::Arc;
use std::time::SystemTime;

use app::trace::Builds;
use kernel::testing::PinnedClock;
use serde_json::json;

use super::{Sandbox, Setup, ToolAnswer, probe, quiet_ports};
use crate::Server;

const SETUP: Setup<'static> = Setup {
    roots: &["repo"],
    files: &[],
    launch: Some("repo"),
};

// A server whose deadline runs on a pinned clock, so no call stops on the time it takes.
fn pinned(sandbox: &Sandbox) -> Server {
    let mut services = (*sandbox.services()).clone();
    services.clock = Arc::new(PinnedClock::at(SystemTime::UNIX_EPOCH));
    sandbox.server_with(&SETUP, Arc::new(services), quiet_ports())
}

fn traced(server: &Server, target: &serde_json::Value) -> ToolAnswer {
    server.answer("trace", json!({ "target": target }).as_object().cloned())
}

// One build of each part of a graph, `times` over.
fn built(times: usize) -> Builds {
    Builds {
        indexes: times,
        scopes: times,
        resolvers: times,
        graphs: times,
    }
}

#[test]
fn a_kept_graph_is_built_once_until_a_file_changes() {
    let sandbox = probe();
    let server = pinned(&sandbox);
    let first = traced(&server, &json!("alpha"));
    let second = traced(&server, &json!("alpha"));
    assert!(!first.is_error, "{}", first.text);
    assert_eq!(second, first);
    assert_eq!(server.inner.graphs.builds(), built(1));
    sandbox.step(&[
        "write",
        "repo/pkg/mod.py",
        "def alpha():\n    return beta()\n\n\ndef beta():\n    return gamma()\n\n\ndef gamma():\n    return 1\n",
    ]);
    let edited = traced(&server, &json!("beta"));
    assert_eq!(server.inner.graphs.builds(), built(2));
    assert_eq!(edited, traced(&pinned(&sandbox), &json!("beta")));
    assert!(edited.text.contains("gamma"), "{}", edited.text);
}

#[test]
fn every_card_of_one_call_reads_one_kept_graph() {
    let sandbox = probe();
    let server = pinned(&sandbox);
    let targets = json!(["alpha", "beta", "pkg/mod.py"]);
    let answer = traced(&server, &targets);
    assert!(!answer.is_error, "{}", answer.text);
    assert_eq!(server.inner.graphs.builds(), built(1));
    assert_eq!(traced(&server, &targets), answer);
    assert_eq!(server.inner.graphs.builds(), built(1));
}

#[test]
fn a_file_card_reads_its_imports_with_no_resolved_graph() {
    let sandbox = probe();
    let server = pinned(&sandbox);
    let answer = traced(&server, &json!("pkg/mod.py"));
    assert!(!answer.is_error, "{}", answer.text);
    assert!(answer.text.contains("main.py"), "{}", answer.text);
    let builds = Builds {
        indexes: 1,
        scopes: 1,
        resolvers: 0,
        graphs: 0,
    };
    assert_eq!(server.inner.graphs.builds(), builds);
}
