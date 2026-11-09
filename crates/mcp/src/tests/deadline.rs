use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use app::Session;
use kernel::testing::PinnedClock;
use serde_json::json;

use super::{Kept, Setup, probe};
use crate::{Arming, Ports};

// The background index of these tests moves the pinned clock, between the call's start and its
// operation.
struct Advancing {
    clock: Arc<PinnedClock>,
    by: Duration,
}

impl Arming for Advancing {
    fn arm(&self, _session: &Session) -> Result<(), String> {
        self.clock.advance(self.by);
        Ok(())
    }

    fn refreshing(&self, _database: &Path) -> bool {
        false
    }
}

const STOPPED: &str = "Error calling tool 'trace': this call stopped after 45 s. The server \
                       allows 30 s of work per call, plus up to 60 s for parsing files the tag \
                       cache does not hold, and this call had 0 s of that grace. Narrow the \
                       request, or run `agentless-mcp index` on the repository so that later \
                       calls read a warm cache. The server flags --call-limit and \
                       --recache-grace set the two bounds.";

#[test]
fn a_call_past_its_limit_stops() {
    let sandbox = probe();
    let clock = Arc::new(PinnedClock::at(SystemTime::UNIX_EPOCH));
    let mut services = (*sandbox.services()).clone();
    services.clock = Arc::clone(&clock) as Arc<dyn kernel::Clock>;
    let ports = Ports {
        arming: Arc::new(Advancing {
            clock,
            by: Duration::from_secs(45),
        }),
        log: Arc::new(Kept::default()),
    };
    let setup = Setup {
        roots: &["repo"],
        files: &[],
        launch: Some("repo"),
    };
    let server = sandbox.server_with(&setup, Arc::new(services), ports);
    let answer = server.answer("trace", json!({"target": "alpha"}).as_object().cloned());
    assert_eq!((answer.text.as_str(), answer.is_error), (STOPPED, true));
}

#[test]
fn an_unknown_tool_is_a_refusal_that_lists_the_tools_and_logs_nothing() {
    let sandbox = probe();
    let log = Arc::new(Kept::default());
    let ports = Ports {
        arming: Arc::new(Advancing {
            clock: Arc::new(PinnedClock::at(SystemTime::UNIX_EPOCH)),
            by: Duration::ZERO,
        }),
        log: Arc::clone(&log) as Arc<dyn crate::Log>,
    };
    let setup = Setup {
        roots: &["repo"],
        files: &[],
        launch: None,
    };
    let server = sandbox.server_with(&setup, sandbox.services(), ports);
    for name in ["orient", "repo_map"] {
        let got = server.answer(name, Some(serde_json::Map::new()));
        let want = format!(
            "Error calling tool '{name}': No tool has this name. The tools are trace and search."
        );
        assert_eq!((got.text, got.is_error), (want, true));
    }
    assert!(log.lines().is_empty(), "{:?}", log.lines());
}
