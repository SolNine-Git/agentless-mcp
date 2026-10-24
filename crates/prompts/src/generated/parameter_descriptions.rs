//! The parameter descriptions.
//! `cargo xtask generate` writes this file from the prompt JSON files.

/// The parameter description keyed `community_resolution`.
pub const COMMUNITY_RESOLUTION: &str = "Larger values generally give smaller communities.";

/// The parameter description keyed `context_lines`.
pub const CONTEXT_LINES: &str = "Lines added around each interval.";

/// The parameter description keyed `diagram_focus`.
pub const DIAGRAM_FOCUS: &str = "Optional file path, path suffix, module name, or symbol name at \
        the centre of a diagram operation. A single string or a list of \
        strings is accepted; a diagram has one centre, so only the \
        first list entry is used.";

/// The parameter description keyed `diagram_max_edges`.
pub const DIAGRAM_MAX_EDGES: &str = "Arrows drawn; the rest are counted.";

/// The parameter description keyed `diagram_max_nodes`.
pub const DIAGRAM_MAX_NODES: &str = "Nodes drawn before an elision node.";

/// The parameter description keyed `docstrings`.
pub const DOCSTRINGS: &str = "Include docstrings.";

/// The parameter description keyed `expand_limit`.
pub const EXPAND_LIMIT: &str = "Maximum symbols expanded in this call; additional requested \
        identifiers are reported as omitted. Omit it to use the tool's \
        default.";

/// The parameter description keyed `explain_limit`.
pub const EXPLAIN_LIMIT: &str = "Maximum rows shown within each fan-in, fan-out, and import \
        section of the explanation. Omit it to use the tool's default.";

/// The parameter description keyed `explain_target`.
pub const EXPLAIN_TARGET: &str = "Stable id, qualified name or name. Ties list the others.";

/// The parameter description keyed `file_path`.
pub const FILE_PATH: &str = "Repository-relative file.";

/// The parameter description keyed `find_limit`.
pub const FIND_LIMIT: &str = "Maximum matching symbol cards returned by find_symbol. Omit it to \
        use the tool's default.";

/// The parameter description keyed `find_name`.
pub const FIND_NAME: &str = "Name, Qualified.name or substring.";

/// The parameter description keyed `group_by_communities`.
pub const GROUP_BY_COMMUNITIES: &str = "Group nodes into community subgraphs.";

/// The parameter description keyed `history_budget`.
pub const HISTORY_BUDGET: &str = "Maximum tokens for commit bodies. Default 12000.";

/// The parameter description keyed `history_limit`.
pub const HISTORY_LIMIT: &str = "Maximum commits. Default 10.";

/// The parameter description keyed `history_target`.
pub const HISTORY_TARGET: &str = "Stable id from map, overview, find, explain or locate. A bare \
        name is refused.";

/// The parameter description keyed `include_ambiguous`.
pub const INCLUDE_AMBIGUOUS: &str = "path: allow name-only edges, where several definitions share \
        the spelling.";

/// The parameter description keyed `include_unique`.
pub const INCLUDE_UNIQUE: &str = "path: allow unique-name edges, which are retrieval evidence, \
        not binding evidence.";

/// The parameter description keyed `locations`.
pub const LOCATIONS: &str = "class:Name, function:name or line:N.";

/// The parameter description keyed `map_budget`.
pub const MAP_BUDGET: &str = "Maximum tokens for the map. Omit it for automatic sizing in a \
        2000-8000 token band.";

/// The parameter description keyed `map_focus`.
pub const MAP_FOCUS: &str = "File paths, path suffixes, module names, or symbol names that \
        receive the map's ranking focus. A single string or a list of \
        strings is accepted; a bare string reads as a one-element list. \
        Use this when the target area is partly known; omit it for \
        repository-wide orientation.";

/// The parameter description keyed `map_granularity`.
pub const MAP_GRANULARITY: &str = "function lists symbols, file lists files alone, body adds \
        budgeted bodies.";

/// The parameter description keyed `map_max_files`.
pub const MAP_MAX_FILES: &str = "Maximum ranked files admitted to the repository map before \
        symbol packing.";

/// The parameter description keyed `no_cache`.
pub const NO_CACHE: &str = "Parse every file live, bypassing the cache.";

/// The parameter description keyed `orient_focus`.
pub const ORIENT_FOCUS: &str = "Paths, path suffixes, module stems or symbol names that seed the \
        ranking. A traceback frame, blob URL or path:line seeds the \
        file it names. diagram uses the first entry.";

/// The parameter description keyed `orient_limit`.
pub const ORIENT_LIMIT: &str = "Maximum rows: map files (default 10), communities, cycles, or \
        rows per health section (default 20).";

/// The parameter description keyed `orient_operation`.
pub const ORIENT_OPERATION: &str = "The tool description says what each one takes.";

/// The parameter description keyed `overview_paths`.
pub const OVERVIEW_PATHS: &str = "Repository-relative files.";

/// The parameter description keyed `path_source`.
pub const PATH_SOURCE: &str = "path: a stable id, qualified name or file path.";

/// The parameter description keyed `path_target`.
pub const PATH_TARGET: &str = "path: a stable id, qualified name or file path.";

/// The parameter description keyed `read_operation`.
pub const READ_OPERATION: &str = "The tool description says what each one takes.";

/// The parameter description keyed `read_path`.
pub const READ_PATH: &str = "Repository-relative path: the file for slice, the subtree for dir.";

/// The parameter description keyed `reference_limit`.
pub const REFERENCE_LIMIT: &str = "Maximum sites, one per file in turn. Default 50.";

/// The parameter description keyed `reference_target`.
pub const REFERENCE_TARGET: &str = "Stable id, qualified name or name.";

/// The parameter description keyed `repo_root`.
pub const REPO_ROOT: &str = "Absolute repository path. Omit it with one root; a refusal lists the \
        roots.";

/// The parameter description keyed `shared_callers`.
pub const SHARED_CALLERS: &str = "Rank symbols whose callers overlap the target's instead: the \
        does-a-utility-already-exist check.";

/// The parameter description keyed `slice_lines`.
pub const SLICE_LINES: &str = "One [start, end] range, or a list of them such as [[10, 40], [90, \
        120]]. 1-based and inclusive.";

/// The parameter description keyed `stable_ids`.
pub const STABLE_IDS: &str = "Stable ids from map, overview, find, explain or locate.";

/// The parameter description keyed `structure_limit`.
pub const STRUCTURE_LIMIT: &str = "Maximum cycles or communities returned, depending on the \
        selected operation. Omit it to use the tool's default.";

/// The parameter description keyed `structure_operation`.
pub const STRUCTURE_OPERATION: &str = "Structural view to compute: path traces precise symbol \
        evidence; cycles reports import SCCs; communities partitions \
        the relevance graph; diagram emits bounded Mermaid; health \
        reports orphan candidates, unused exports, hubs and complexity \
        hotspots.";

/// The parameter description keyed `symbol_kind`.
pub const SYMBOL_KIND: &str = "Symbol kind filter.";

/// The parameter description keyed `symbols_limit`.
pub const SYMBOLS_LIMIT: &str = "Maximum rows: find and explain default 20, expand 10.";

/// The parameter description keyed `symbols_operation`.
pub const SYMBOLS_OPERATION: &str = "The tool description says what each one requires.";

/// The parameter description keyed `tree_depth`.
pub const TREE_DEPTH: &str = "Directory levels below the tree root.";

/// The parameter description keyed `tree_max_entries`.
pub const TREE_MAX_ENTRIES: &str = "Entries listed before the tree reports truncation.";

/// The parameter description keyed `tree_path`.
pub const TREE_PATH: &str = "Optional repository-relative directory whose tracked subtree should \
        be rendered. Omit it to render from the repository root.";

/// The parameter description keyed `whole_file`.
pub const WHOLE_FILE: &str = "The whole file, in place of lines.";
