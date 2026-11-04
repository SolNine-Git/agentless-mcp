//! One home for each default and bound of the read operations. The operations check these. The
//! help texts in table.rs spell the same values, and a test holds them equal.

/// The most rows that one listing parameter may ask for.
pub const MAX_LIMIT: usize = 500;
/// The most context lines around a slice or a resolved location.
pub const MAX_CONTEXT_LINES: usize = 200;
/// The default context lines around a slice or a resolved location.
pub const DEFAULT_CONTEXT_LINES: usize = facts::DEFAULT_CONTEXT_LINES as usize;

/// The smallest token budget of the map and of the history bodies: the config file's range.
pub const MIN_BUDGET: usize = io::MIN_BUDGET as usize;
/// The largest token budget.
pub const MAX_BUDGET: usize = io::MAX_BUDGET as usize;
/// The fewest ranked files a map admits: the config file's range.
pub const MIN_MAX_FILES: usize = io::MIN_MAX_FILES as usize;
/// The most ranked files a map admits.
pub const MAX_MAX_FILES: usize = io::MAX_MAX_FILES as usize;
/// The map's file count when neither the call nor the config sets it.
pub const DEFAULT_MAX_FILES: usize = 10;
/// The most test companions a map lists.
pub const MAX_TEST_COMPANIONS: usize = 5;
/// The hops the walk to a map's test companions takes from the ranked and seeded files.
pub const TEST_COMPANION_DEPTH: u32 = 2;

/// The tree's default depth.
pub const DEFAULT_TREE_DEPTH: usize = 4;
/// The tree's default entry count.
pub const DEFAULT_TREE_ENTRIES: usize = 500;
/// The deepest tree, which is the walk's depth bound.
pub const MAX_TREE_DEPTH: usize = io::MAX_WALK_DEPTH;
/// The most tree entries, which is the walk's file bound.
pub const MAX_TREE_ENTRIES: usize = io::MAX_WALK_FILES;

/// The default `expand` limit.
pub const DEFAULT_EXPAND_LIMIT: usize = 10;
/// The default `find-symbol` limit.
pub const DEFAULT_FIND_LIMIT: usize = 20;
/// The most usage files that `find-symbol` lists for one name before it counts the rest.
pub const FIND_USAGE_FILES: usize = 10;
/// The default `refs` limit.
pub const DEFAULT_REFS_LIMIT: usize = 50;
/// The default `history` limit.
pub const DEFAULT_HISTORY_LIMIT: usize = 10;
/// The default `history` body budget.
pub const DEFAULT_HISTORY_BUDGET: usize = 12_000;
/// The default `explain` limit.
pub const DEFAULT_EXPLAIN_LIMIT: usize = 20;
/// The default `cycles` limit.
pub const DEFAULT_CYCLE_LIMIT: usize = 20;
/// The default `communities` limit.
pub const DEFAULT_COMMUNITY_LIMIT: usize = 20;
/// The default member count per community.
pub const DEFAULT_MEMBER_LIMIT: usize = 12;
/// The default `health` limit.
pub const DEFAULT_HEALTH_LIMIT: usize = 20;
/// The default node bound of a path search.
pub const DEFAULT_MAX_VISITED: usize = graph::DEFAULT_MAX_VISITED;

/// The default diagram node count.
pub const DEFAULT_DIAGRAM_NODES: usize = 40;
/// The default diagram edge count.
pub const DEFAULT_DIAGRAM_EDGES: usize = 40;
/// The most diagram nodes.
pub const MAX_DIAGRAM_NODES: usize = 500;
/// The most diagram edges.
pub const MAX_DIAGRAM_EDGES: usize = 500;
/// The default HTML export node count.
pub const DEFAULT_HTML_NODES: usize = 200;
/// The default HTML export edge count.
pub const DEFAULT_HTML_EDGES: usize = 600;
/// The most HTML export nodes.
pub const MAX_HTML_NODES: usize = 1_000;
/// The most HTML export edges.
pub const MAX_HTML_EDGES: usize = 5_000;

/// The default community resolution.
pub const DEFAULT_RESOLUTION: f64 = graph::DEFAULT_RESOLUTION;
/// The highest community resolution.
pub const MAX_RESOLUTION: f64 = 100.0;
