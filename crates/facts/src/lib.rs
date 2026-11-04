//! The symbols, imports, identifier occurrences and error spans of one parse, their stable ids,
//! and the skeleton, slice and location views over them.

mod declared;
mod extract;
mod locs;
mod mentions;
mod pattern;
mod skeleton;
mod slices;
mod spelling;
mod stable_id;
mod values;

pub use declared::Declared;
pub use extract::{DataOutline, OUTLINE_CAP, OutlineRow, data_outline, extract, is_data_language};
pub use locs::{
    DEFAULT_CONTEXT_LINES, FileSymbols, LocResolution, LocTarget, UnrecognizedLoc, resolve_locs,
};
pub use mentions::unparsed_mentions;
pub use skeleton::{SkeletonOptions, skeletonize};
pub use slices::{OutsideFile, line_count, line_prefix, slice_lines};
pub use spelling::base_name;
pub use stable_id::{StableId, StableIdError, rationale_handle, split_ordinal};
pub use values::{Facts, Import, Rationale, RationaleKind, Ref, Role, Symbol, SymbolKind};
