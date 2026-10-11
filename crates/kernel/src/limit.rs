use serde::de::{Deserializer, Error as _};
use serde::{Deserialize, Serialize};

/// A count in `MIN..=MAX` that defaults to `DEF` and refuses an out-of-range value when parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Limit<const MIN: u32, const MAX: u32, const DEF: u32>(u32);

impl<const MIN: u32, const MAX: u32, const DEF: u32> Limit<MIN, MAX, DEF> {
    /// Return the limit for `value`, or `None` when `value` is outside `MIN..=MAX`.
    #[must_use]
    pub const fn new(value: u32) -> Option<Self> {
        if MIN <= value && value <= MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Return the count.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl<const MIN: u32, const MAX: u32, const DEF: u32> Default for Limit<MIN, MAX, DEF> {
    fn default() -> Self {
        // A default outside its own range fails the build of any type that uses it.
        const { assert!(MIN <= DEF && DEF <= MAX, "DEF is outside MIN..=MAX") };
        Self(DEF)
    }
}

impl<'de, const MIN: u32, const MAX: u32, const DEF: u32> Deserialize<'de>
    for Limit<MIN, MAX, DEF>
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u32::deserialize(deserializer)?;
        Self::new(value)
            .ok_or_else(|| D::Error::custom(format!("{value} is outside the range {MIN} to {MAX}")))
    }
}

#[cfg(test)]
mod tests {
    use super::Limit;
    use serde::Deserialize;

    type Rows = Limit<1, 500, 10>;

    #[derive(Debug, Deserialize)]
    struct Request {
        #[serde(default)]
        limit: Rows,
    }

    #[test]
    fn the_bounds_are_inclusive() {
        assert_eq!(Rows::new(1).map(Rows::get), Some(1));
        assert_eq!(Rows::new(500).map(Rows::get), Some(500));
    }

    #[test]
    fn a_value_outside_the_bounds_is_refused_when_parsed() {
        for text in ["0", "501"] {
            let refused = serde_json::from_str::<Rows>(text).unwrap_err();
            assert_eq!(
                refused.to_string(),
                format!("{text} is outside the range 1 to 500")
            );
        }
    }

    #[test]
    fn an_omitted_limit_takes_the_default() {
        let request: Request = serde_json::from_str("{}").unwrap();
        assert_eq!(request.limit.get(), 10);
    }

    #[test]
    fn a_limit_round_trips_as_a_bare_number() {
        let limit = Rows::new(42).unwrap();
        let text = serde_json::to_string(&limit).unwrap();
        assert_eq!(text, "42");
        assert_eq!(serde_json::from_str::<Rows>(&text).unwrap(), limit);
    }
}
