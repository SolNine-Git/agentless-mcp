//! The JSON key of each field of a finding's detail and of the observed state. The erosion gate
//! log stores these rows, and its report reads them back by the same names.

/// The observed units with complexity above the threshold.
pub const UNITS_OVER_THRESHOLD: &str = "units_over_threshold";
/// The observed units that a change added or altered.
pub const UNITS_CHANGED: &str = "units_changed";
/// The observed contract hits.
pub const CONTRACT_KEYS: &str = "contract_keys";
/// The observed blocks that `clone_delta` could report.
pub const CLONE_INSTANCES: &str = "clone_instances";

/// A repo-relative path.
pub const PATH: &str = "path";
/// A unit's qualified name.
pub const UNIT: &str = "unit";
/// A first line.
pub const LINE: &str = "line";
/// A last line.
pub const END_LINE: &str = "end_line";
/// A complexity.
pub const CC: &str = "cc";
/// A source line count.
pub const SLOC: &str = "sloc";
/// How a new unit was paired with an old one.
pub const MATCH: &str = "match";
/// The old complexity.
pub const CC_OLD: &str = "cc_old";
/// The new complexity.
pub const CC_NEW: &str = "cc_new";
/// The old source line count.
pub const SLOC_OLD: &str = "sloc_old";
/// The new source line count.
pub const SLOC_NEW: &str = "sloc_new";
/// The complexity threshold of the run.
pub const THRESHOLD: &str = "threshold";
/// A contract rule id.
pub const RULE: &str = "rule";
/// A contract hit's normalized text.
pub const CAPTURE: &str = "capture";
/// A block's Type-2 key.
pub const KEY: &str = "key";
/// A copied block's first other instances.
pub const SITES: &str = "sites";
/// The number of a copied block's other instances.
pub const SITES_TOTAL: &str = "sites_total";

// Serializes each named struct as one JSON object: each field under its key const, in order.
macro_rules! keyed {
    ($($name:ident { $($key:ident: $field:ident),+ $(,)? })+) => {$(
        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeStruct;
                let fields = [$(stringify!($field)),+].len();
                let mut row = serializer.serialize_struct(stringify!($name), fields)?;
                $(row.serialize_field($crate::schema::$key, &self.$field)?;)+
                row.end()
            }
        }
    )+};
}

pub(crate) use keyed;
