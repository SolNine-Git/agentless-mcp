use super::{allocate, longest_fitting};

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");
const FIXTURE: &str = "read_symbols/budget";

#[test]
fn allocate_cuts_as_the_reference_cuts() {
    let cases: Vec<(Vec<usize>, usize, Vec<usize>, usize)> =
        io::testing::cases(MANIFEST_DIR, FIXTURE, "allocate");
    assert!(cases.len() >= 200, "{} cases", cases.len());
    for (costs, budget, cut, share) in cases {
        let (got_cut, got_share) = allocate(&costs, budget);
        let got_cut: Vec<usize> = got_cut.into_iter().collect();
        assert_eq!((got_cut, got_share), (cut, share), "{costs:?} in {budget}");
    }
}

#[test]
fn the_longest_fit_is_the_last_count_that_fits_or_the_floor() {
    assert_eq!(longest_fitting(1, 10, |count| count <= 4), 4);
    assert_eq!(longest_fitting(1, 10, |_| false), 1);
    assert_eq!(longest_fitting(0, 10, |_| false), 0);
    assert_eq!(longest_fitting(0, 10, |_| true), 10);
    assert_eq!(longest_fitting(3, 3, |_| false), 3);
}
