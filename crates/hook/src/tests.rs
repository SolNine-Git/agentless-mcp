use std::time::{Instant, SystemTime, UNIX_EPOCH};

use app::Services;
use app::hooks::{Hook, Retired};
use kernel::Clock;
use prompts::messages::HOOK_USAGE;

use crate::testing::Sandbox;
use crate::{Output, erosion_with, watched};

// A clock that fails before any step runs, so only the outer guard can catch it.
struct BrokenClock;

impl Clock for BrokenClock {
    fn now(&self) -> Instant {
        panic!("the clock broke")
    }

    fn wall(&self) -> SystemTime {
        UNIX_EPOCH
    }
}

#[test]
fn a_panic_outside_the_steps_still_exits_0() {
    let sandbox = Sandbox::new();
    let env = sandbox.env(&[]);
    let services = Services::new(&env);

    let output = erosion_with(&env, &services, &mut &b""[..], Box::new(BrokenClock));

    let expected = Output {
        stderr: "erosion gate internal error: panic: the clock broke\n".to_owned(),
        ..Output::default()
    };
    assert_eq!(output, expected);
}

#[test]
fn a_sigterm_watch_that_fails_is_named_first_on_stderr() {
    let output = Output {
        stderr: "the gate's own line\n".to_owned(),
        ..Output::default()
    };

    let output = watched(output, Err(std::io::Error::from_raw_os_error(24)));

    let note = "erosion gate cannot watch SIGTERM: Too many open files (os error 24)\n";
    assert_eq!(output.stderr, format!("{note}the gate's own line\n"));
    assert_eq!(output.code, 0);
}

#[test]
fn the_hook_usage_names_every_current_hook_and_each_retired_one() {
    let current = Hook::ALL.map(Hook::name);
    let retired = Retired::ALL.map(Retired::name);
    for name in current.into_iter().chain(retired) {
        assert!(HOOK_USAGE.contains(name), "{name}: {HOOK_USAGE}");
    }
    assert!(HOOK_USAGE.contains("exits 0"), "{HOOK_USAGE}");
}
