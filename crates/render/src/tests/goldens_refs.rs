use app::RefsRequest;

use super::goldens_read::{Pinned, golden, int};

// The target of each repository's refs golden, as the golden harness names it.
const CASES: [(&str, &str); 3] = [
    ("repo_py", "format_money"),
    ("repo_ts", "formatMoney"),
    ("repo_nested", "format_amount"),
];

#[test]
fn the_refs_goldens_come_out_byte_for_byte() {
    for (repo, target) in CASES {
        let pinned = Pinned::new(repo);
        let request = RefsRequest {
            target: target.to_owned(),
            limit: int("50"),
            shared_callers: false,
        };
        let noted = app::refs(&pinned.services, &pinned.reading(), &request);
        assert!(
            noted.diagnostics.is_empty(),
            "{repo}: {:?}",
            noted.diagnostics
        );
        let result = noted
            .value
            .unwrap_or_else(|error| panic!("{repo}: {error}"));
        assert_eq!(
            pinned.text(&result),
            golden("graph", repo, "refs.txt"),
            "{repo}"
        );
    }
}
