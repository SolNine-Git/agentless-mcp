/// Rows cut to a limit, with the total they were cut from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bounded<T> {
    rows: Vec<T>,
    total: usize,
    limit: usize,
}

impl<T> Bounded<T> {
    /// Keep the first `limit` of `rows`, and remember how many there were.
    #[must_use]
    pub fn cut(mut rows: Vec<T>, limit: usize) -> Self {
        let total = rows.len();
        rows.truncate(limit);
        Self { rows, total, limit }
    }

    /// Keep the first `limit` of `rows`, which a caller chose from `total` rows, with that total.
    ///
    /// # Panics
    ///
    /// Panics when `rows` holds more than `total` rows.
    #[must_use]
    pub fn with_total(mut rows: Vec<T>, total: usize, limit: usize) -> Self {
        assert!(
            rows.len() <= total,
            "{} rows came from a total of {total}",
            rows.len()
        );
        rows.truncate(limit);
        Self { rows, total, limit }
    }

    /// Return the rows the limit kept.
    #[must_use]
    pub fn rows(&self) -> &[T] {
        &self.rows
    }

    /// Return how many rows there were before the cut.
    #[must_use]
    pub const fn total(&self) -> usize {
        self.total
    }

    /// Return the limit that made the cut.
    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    /// Return how many rows the limit left out.
    #[must_use]
    pub fn omitted(&self) -> usize {
        self.total - self.rows.len()
    }
}

#[cfg(test)]
mod tests {
    use super::Bounded;

    #[test]
    fn a_cut_keeps_the_first_rows_and_counts_the_rest() {
        let bounded = Bounded::cut(vec!["a", "b", "c"], 2);
        assert_eq!(bounded.rows(), ["a", "b"]);
        assert_eq!(
            (bounded.total(), bounded.limit(), bounded.omitted()),
            (3, 2, 1)
        );
    }

    #[test]
    fn a_limit_above_the_total_omits_nothing() {
        let bounded = Bounded::cut(vec![1, 2], 10);
        assert_eq!(bounded.rows(), [1, 2]);
        assert_eq!(bounded.omitted(), 0);
    }

    #[test]
    fn a_zero_limit_omits_every_row() {
        let bounded = Bounded::cut(vec![1, 2, 3], 0);
        assert!(bounded.rows().is_empty());
        assert_eq!(bounded.omitted(), 3);
    }

    #[test]
    fn chosen_rows_keep_the_total_they_were_chosen_from() {
        let bounded = Bounded::with_total(vec!["a", "b"], 5, 2);
        assert_eq!(bounded.rows(), ["a", "b"]);
        assert_eq!(
            (bounded.total(), bounded.limit(), bounded.omitted()),
            (5, 2, 3)
        );
        assert_eq!(
            Bounded::with_total(vec![1, 2, 3], 3, 10),
            Bounded::cut(vec![1, 2, 3], 10)
        );
    }

    #[test]
    fn chosen_rows_past_the_limit_are_cut() {
        let bounded = Bounded::with_total(vec![1, 2, 3], 4, 2);
        assert_eq!(bounded.rows(), [1, 2]);
        assert_eq!(bounded.omitted(), 2);
    }

    #[test]
    #[should_panic(expected = "3 rows came from a total of 2")]
    fn more_rows_than_the_total_is_a_caller_error() {
        let _ = Bounded::with_total(vec![1, 2, 3], 2, 5);
    }
}
