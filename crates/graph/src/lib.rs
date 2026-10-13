//! The repository graphs: the name index, the import scopes, the resolver, the symbol and file
//! graphs, their ranks, floods, paths, cycles and communities.

mod ambiguous;
mod communities;
mod cycles;
mod files;
mod flood;
mod imports;
mod lookup;
mod names;
mod owners;
mod path;
mod paths;
mod rank;
mod resolver;
mod scan;
mod scopes;
mod symbols;
mod unresolved;

pub use ambiguous::AmbiguousDegrees;
pub use communities::{
    Community, CommunityPartition, DEFAULT_RESOLUTION, ROOT_LABEL, community_label,
    detect_communities,
};
pub use cycles::{Cycle, import_cycles};
pub use files::{FileGraph, Weighting};
pub use flood::{Flood, Reached, flood_backward};
pub use lookup::{Lookup, definitions_for, references_to, resolve_definitions};
pub use names::{Definition, NameIndex, Site, common_name_damping, name_multiplier};
pub use owners::{LineOwners, enclosing_symbol};
pub use path::{DEFAULT_MAX_VISITED, Hop, PathPolicy, PathResult, shortest_path};
pub use rank::{PageRank, pagerank, rank_order};
pub use resolver::{Resolution, Resolver, Tier};
pub use scan::{FileFacts, in_name_graph};
pub use scopes::{ImportScope, Scopes};
pub use symbols::{AmbiguousReference, Endpoint, Relation, SymbolEdge, SymbolGraph};

#[cfg(test)]
mod tests;
