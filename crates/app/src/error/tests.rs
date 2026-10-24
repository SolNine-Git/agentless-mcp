use kernel::Stopped;

use super::Error;

const ADVICE: &str = "Narrow the request, or run `agentless-mcp index` on the repository so \
                      that later calls read a warm cache. The server flags --call-limit and \
                      --recache-grace set the two bounds.";

fn message(stopped: Stopped) -> String {
    match Error::from(stopped) {
        Error::Kind(kernel::Error::CallStopped(message)) => message,
        other => panic!("not a stopped call: {other:?}"),
    }
}

fn spent(elapsed: f64, limit: f64, grace: f64, credited: f64) -> Stopped {
    Stopped::Spent {
        elapsed,
        limit,
        grace,
        credited,
    }
}

// The reference texts: half-even whole seconds for the elapsed and credited times, and the
// general form for the two bounds.
#[test]
fn a_spent_call_stops_with_the_reference_text() {
    let cases = [
        (spent(12.5, 5.0, 7.0, 0.0), "12", "5", "7", "0"),
        (spent(95.5, 30.0, 60.0, 60.0), "96", "30", "60", "60"),
        (spent(13.5, 0.5, 1e6, 2.5), "14", "0.5", "1e+06", "2"),
    ];
    for (stopped, elapsed, limit, grace, credited) in cases {
        let expected = format!(
            "this call stopped after {elapsed} s. The server allows {limit} s of work per call, \
             plus up to {grace} s for parsing files the tag cache does not hold, and this call \
             had {credited} s of that grace. {ADVICE}"
        );
        assert_eq!(message(stopped), expected);
    }
}

#[test]
fn a_cancelled_call_stops_with_the_reference_text() {
    assert_eq!(
        message(Stopped::Cancelled),
        "the client cancelled this call, so it stopped at its next checkpoint"
    );
}
