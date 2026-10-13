use super::{CommunitiesRequest, CommunityReport, communities, resolution_setting};
use crate::endpoints::scanned::{LINKED, Scanned};
use crate::{Error, IntArg};

fn int(text: &str) -> IntArg {
    IntArg::parse(text).unwrap_or_else(|| panic!("{text:?}"))
}

fn partition(
    resolution: Option<f64>,
    limit: &str,
    members: &str,
) -> Result<CommunityReport, Error> {
    let scanned = Scanned::new(&LINKED);
    let request = CommunitiesRequest {
        resolution,
        limit: int(limit),
        members: int(members),
    };
    communities(&scanned.services, &scanned.reading(), &request).value
}

fn refusal(outcome: Result<impl std::fmt::Debug, Error>) -> String {
    match outcome {
        Err(Error::Kind(kernel::Error::OperationFailed(message))) => message,
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_resolution_is_finite_above_zero_and_at_most_one_hundred() {
    assert_eq!(resolution_setting(None).ok(), Some(1.0));
    assert_eq!(resolution_setting(Some(0.25)).ok(), Some(0.25));
    let cases = [
        (
            f64::NAN,
            "resolution must be a finite number greater than 0, got nan",
        ),
        (
            f64::INFINITY,
            "resolution must be a finite number greater than 0, got inf",
        ),
        (
            0.0,
            "resolution must be a finite number greater than 0, got 0.0",
        ),
        (
            -1.0,
            "resolution must be a finite number greater than 0, got -1.0",
        ),
        (
            100.5,
            "resolution takes a value from 0.0 through 100.0, got 100.5",
        ),
    ];
    for (value, expected) in cases {
        assert_eq!(
            refusal(resolution_setting(Some(value))),
            expected,
            "{value}"
        );
    }
}

#[test]
fn the_two_counts_are_checked_before_the_scan() {
    let limit = "limit takes a value from 1 through 500, got 0";
    assert_eq!(refusal(partition(None, "0", "1")), limit);
    let members = "members must be at least 1, got 0";
    assert_eq!(refusal(partition(None, "1", "0")), members);
}

#[test]
fn a_member_cut_keeps_the_community_size() {
    let report = partition(None, "20", "1").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(report.files, 4);
    let sizes: usize = report
        .communities
        .rows()
        .iter()
        .map(|row| row.members.total())
        .sum();
    assert_eq!(sizes, 4);
    assert!(
        report
            .communities
            .rows()
            .iter()
            .all(|row| row.members.rows().len() == 1)
    );
}
