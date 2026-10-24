use serde::{Deserialize, Serialize};

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
    use super::FileId;

    #[test]
    fn an_id_round_trips_as_a_bare_number() {
        let id = FileId::new(7);
        let text = serde_json::to_string(&id).unwrap();
        assert_eq!(text, "7");
        assert_eq!(serde_json::from_str::<FileId>(&text).unwrap().get(), 7);
    }
}
