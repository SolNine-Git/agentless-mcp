use serde::{Deserialize, Serialize};

/// A byte range in one source text: `start` inclusive, `end` exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "SpanParts")]
pub struct Span {
    start: u32,
    end: u32,
}

#[derive(Deserialize)]
struct SpanParts {
    start: u32,
    end: u32,
}

impl TryFrom<SpanParts> for Span {
    type Error = String;

    fn try_from(parts: SpanParts) -> Result<Self, Self::Error> {
        Self::new(parts.start, parts.end).ok_or_else(|| {
            format!(
                "a span ends before it starts: {}..{}",
                parts.start, parts.end
            )
        })
    }
}

impl Span {
    /// Return the span from `start` to `end`, or `None` when it ends before it starts.
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Option<Self> {
        if start <= end {
            Some(Self { start, end })
        } else {
            None
        }
    }

    /// Return the first byte offset.
    #[must_use]
    pub const fn start(self) -> u32 {
        self.start
    }

    /// Return the offset one past the last byte.
    #[must_use]
    pub const fn end(self) -> u32 {
        self.end
    }
}

macro_rules! typed_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(u32);

        impl $name {
            /// Return the id for `index`.
            #[must_use]
            pub const fn new(index: u32) -> Self {
                Self(index)
            }

            /// Return the index.
            #[must_use]
            pub const fn get(self) -> u32 {
                self.0
            }
        }
    };
}

typed_id!(
    /// A file's index in one repository listing.
    FileId
);
typed_id!(
    /// An interned name's index.
    NameId
);
typed_id!(
    /// A node's index in one parse.
    NodeId
);

#[cfg(test)]
mod tests {
    use super::{FileId, Span};

    #[test]
    fn a_span_round_trips_and_keeps_its_ends() {
        let span = Span::new(3, 9).unwrap();
        let text = serde_json::to_string(&span).unwrap();
        assert_eq!(text, r#"{"start":3,"end":9}"#);
        assert_eq!(serde_json::from_str::<Span>(&text).unwrap(), span);
        assert_eq!((span.start(), span.end()), (3, 9));
    }

    #[test]
    fn a_span_that_ends_before_it_starts_is_refused() {
        assert_eq!(Span::new(9, 3), None);
        let refused = serde_json::from_str::<Span>(r#"{"start":9,"end":3}"#).unwrap_err();
        assert!(
            refused
                .to_string()
                .starts_with("a span ends before it starts: 9..3")
        );
    }

    #[test]
    fn an_id_round_trips_as_a_bare_number() {
        let id = FileId::new(7);
        let text = serde_json::to_string(&id).unwrap();
        assert_eq!(text, "7");
        assert_eq!(serde_json::from_str::<FileId>(&text).unwrap().get(), 7);
    }
}
