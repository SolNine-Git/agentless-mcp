use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use app::Session;
use kernel::testing::PinnedClock;
use serde_json::json;

use super::calls::{answered, built, reference, run_named};
use super::{Kept, Setup};
use crate::dispatch::UNPLANNED;
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

const STOPPED: &str = "Error calling tool 'symbols': this call stopped after 45 s. The server \
                       allows 30 s of work per call, plus up to 60 s for parsing files the tag \
                       cache does not hold, and this call had 0 s of that grace. Narrow the \
                       request, or run `agentless-mcp index` on the repository so that later \
                       calls read a warm cache. The server flags --call-limit and \
                       --recache-grace set the two bounds.";

#[test]
fn a_call_past_its_limit_stops_and_capabilities_has_no_limit() {
    let run = run_named("probe");
    let sandbox = built(&run);
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
    let find = json!({"operation": "find", "name": "alpha"});
    assert_eq!(
        answered(&sandbox, &server, "symbols", &find),
        (STOPPED.to_owned(), true)
    );
    let want = (reference(&run, "capabilities", &json!({})), false);
    assert_eq!(
        answered(&sandbox, &server, "capabilities", &json!({})),
        want
    );
}

#[test]
fn an_unknown_tool_is_an_unplanned_failure_with_its_detail_in_the_log() {
    let run = run_named("probe");
    let sandbox = built(&run);
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
    let got = answered(&sandbox, &server, "repo_map", &json!({}));
    assert_eq!(got, (UNPLANNED.to_owned(), true));
    let lines = log.lines();
    assert_eq!(
        lines,
        ["unhandled error in an agentless-mcp tool call: repo_map: no tool repo_map"]
    );
}
