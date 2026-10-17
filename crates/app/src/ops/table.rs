//! The declarations of the read operations, in the order the command line lists them.

use super::limits::{
    DEFAULT_COMMUNITY_LIMIT, DEFAULT_CONTEXT_LINES, DEFAULT_CYCLE_LIMIT, DEFAULT_DIAGRAM_EDGES,
    DEFAULT_DIAGRAM_NODES, DEFAULT_EXPAND_LIMIT, DEFAULT_EXPLAIN_LIMIT, DEFAULT_FIND_LIMIT,
    DEFAULT_HEALTH_LIMIT, DEFAULT_HISTORY_BUDGET, DEFAULT_HISTORY_LIMIT, DEFAULT_HTML_EDGES,
    DEFAULT_HTML_NODES, DEFAULT_MAX_VISITED, DEFAULT_MEMBER_LIMIT, DEFAULT_REFS_LIMIT,
    DEFAULT_TREE_DEPTH, DEFAULT_TREE_ENTRIES,
};
use super::{Arity, Cli, Door, Op, Param, Value};

/// The `--repo` option of every repository command.
pub const REPO: Param = text(
    "repo",
    &["--repo"],
    Some("PATH"),
    Door::Named("repo_root"),
    Some("repository root (default: the git root enclosing the current directory)"),
);
/// The `--json` option of every repository command.
pub const JSON: Param = flag(
    "json",
    &["--json"],
    Door::CliOnly,
    "emit JSON instead of text",
);
/// The `--no-cache` option of every repository command.
pub const NO_CACHE: Param = flag(
    "no_cache",
    &["--no-cache"],
    Door::Same,
    "parse on demand and ignore the tag cache for this call",
);
// The same option where the MCP tool has no such parameter.
const NO_CACHE_CLI_ONLY: Param = Param {
    door: Door::CliOnly,
    ..NO_CACHE
};

// The kind names `find-symbol --kind` accepts, in facts::SymbolKind::ALL order.
pub(crate) const KIND_NAMES: [&str; 9] = [
    "class",
    "function",
    "method",
    "constant",
    "type_alias",
    "protocol",
    "dataclass",
    "enum",
    "decorator",
];
// The map granularities, in reference order.
pub(crate) const GRANULARITY_NAMES: [&str; 3] = ["function", "file", "body"];

const fn flag(
    dest: &'static str,
    flags: &'static [&'static str],
    door: Door,
    help: &'static str,
) -> Param {
    Param {
        dest,
        cli: Some(option(flags, None)),
        door,
        value: Value::Flag,
        help: Some(help),
    }
}

const fn text(
    dest: &'static str,
    flags: &'static [&'static str],
    metavar: Option<&'static str>,
    door: Door,
    help: Option<&'static str>,
) -> Param {
    Param {
        dest,
        cli: Some(option(flags, metavar)),
        door,
        value: Value::Text,
        help,
    }
}

const fn int(
    dest: &'static str,
    flags: &'static [&'static str],
    default: usize,
    door: Door,
    help: Option<&'static str>,
) -> Param {
    Param {
        dest,
        cli: Some(option(flags, None)),
        door,
        value: Value::Int(default),
        help,
    }
}

const fn float(dest: &'static str, flags: &'static [&'static str], help: &'static str) -> Param {
    Param {
        dest,
        cli: Some(option(flags, None)),
        door: Door::Same,
        value: Value::Float,
        help: Some(help),
    }
}

const fn positional(dest: &'static str, metavar: &'static str, arity: Arity, door: Door) -> Param {
    Param {
        dest,
        cli: Some(Cli::Positional { metavar, arity }),
        door,
        value: Value::Text,
        help: None,
    }
}

const fn option(flags: &'static [&'static str], metavar: Option<&'static str>) -> Cli {
    Cli::Option {
        flags,
        metavar,
        repeat: false,
        required: false,
    }
}

// A parameter that only the MCP tool takes.
const fn mcp_only(dest: &'static str, value: Value) -> Param {
    Param {
        dest,
        cli: None,
        door: Door::Same,
        value,
        help: None,
    }
}

/// The `map` operation.
pub static MAP: Op = Op {
    name: "map",
    summary: "ranked, token-budgeted repository map",
    tool: Some("repo_map"),
    params: &[
        REPO,
        JSON,
        NO_CACHE,
        Param {
            dest: "focus",
            cli: Some(Cli::Option {
                flags: &["--focus"],
                metavar: Some("FILE_OR_SYMBOL"),
                repeat: true,
                required: false,
            }),
            door: Door::Same,
            value: Value::Text,
            help: Some("seed the ranking with a file or symbol; repeatable"),
        },
        text(
            "budget",
            &["--budget"],
            None,
            Door::Same,
            Some(
                "token budget for the map body, or 'auto' to size it from the repository \
                     (default: auto)",
            ),
        ),
        Param {
            value: Value::IntOrUnset,
            ..int(
                "max_files",
                &["--max-files"],
                0,
                Door::Same,
                Some("ranked files admitted to the map before symbol packing (default: 10)"),
            )
        },
        Param {
            value: Value::Choice(&GRANULARITY_NAMES),
            ..text(
                "granularity",
                &["--granularity"],
                None,
                Door::Same,
                Some(
                    "map detail: 'function' lists symbols within ranked files, 'file' \
                         reports ranked files only, 'body' returns the file rows plus the top \
                         symbols' budgeted bodies (default: function)",
                ),
            )
        },
    ],
    exclusive: &[],
};

/// The `tree` operation.
pub static TREE: Op = Op {
    name: "tree",
    summary: "gitignore-aware directory tree",
    tool: Some("list_dir"),
    params: &[
        REPO,
        JSON,
        NO_CACHE_CLI_ONLY,
        mcp_only("path", Value::Text),
        int(
            "depth",
            &["--depth"],
            DEFAULT_TREE_DEPTH,
            Door::Same,
            Some("directory levels rendered below the tree root (default: 4)"),
        ),
        int(
            "max_entries",
            &["--max-entries"],
            DEFAULT_TREE_ENTRIES,
            Door::Same,
            Some(
                "files and directories rendered before the tree reports truncation \
                     (default: 500)",
            ),
        ),
    ],
    exclusive: &[],
};

/// The `skeleton` operation.
pub static SKELETON: Op = Op {
    name: "skeleton",
    summary: "signatures with bodies elided",
    tool: Some("get_symbols_overview"),
    params: &[
        REPO,
        JSON,
        NO_CACHE,
        positional("files", "FILE", Arity::OneOrMore, Door::Named("paths")),
        Param {
            value: Value::FlagOrUnset,
            ..flag(
                "docstrings",
                &["--docstrings"],
                Door::Same,
                "keep docstrings, truncated (off by default: tokens and injection surface)",
            )
        },
        flag(
            "numbers",
            &["--numbers"],
            Door::CliOnly,
            "prefix lines with N|",
        ),
    ],
    exclusive: &[],
};

/// The `expand` operation.
pub static EXPAND: Op = Op {
    name: "expand",
    summary: "full bodies for named stable ids",
    tool: Some("expand_symbols"),
    params: &[
        REPO,
        JSON,
        NO_CACHE,
        positional(
            "ids",
            "STABLE_ID",
            Arity::OneOrMore,
            Door::Named("stable_ids"),
        ),
        int(
            "limit",
            &["--limit"],
            DEFAULT_EXPAND_LIMIT,
            Door::Same,
            None,
        ),
    ],
    exclusive: &[],
};

/// The `slice` operation.
pub static SLICE: Op = Op {
    name: "slice",
    summary: "numbered lines with scope headers",
    tool: Some("read_slice"),
    params: &[
        REPO,
        JSON,
        NO_CACHE_CLI_ONLY,
        positional("file", "FILE", Arity::Optional, Door::Named("path")),
        text(
            "symbol",
            &["--symbol"],
            Some("STABLE_ID"),
            Door::CliOnly,
            Some("slice this symbol instead"),
        ),
        Param {
            dest: "lines",
            cli: Some(Cli::Option {
                flags: &["--lines"],
                metavar: Some("A:B"),
                repeat: true,
                required: false,
            }),
            door: Door::Same,
            value: Value::Text,
            help: Some("1-based inclusive line range; repeatable and merged; needs FILE"),
        },
        int(
            "context",
            &["--context"],
            DEFAULT_CONTEXT_LINES,
            Door::Named("context_lines"),
            None,
        ),
        mcp_only("whole_file", Value::Flag),
    ],
    exclusive: &["file", "symbol"],
};

/// The `find-symbol` operation.
pub static FIND_SYMBOL: Op = Op {
    name: "find-symbol",
    summary: "substring or qualified symbol lookup",
    tool: Some("find_symbol"),
    params: &[
        REPO,
        JSON,
        NO_CACHE,
        positional("name", "NAME", Arity::One, Door::Same),
        Param {
            value: Value::Choice(&KIND_NAMES),
            ..text("kind", &["--kind"], None, Door::Same, None)
        },
        int("limit", &["--limit"], DEFAULT_FIND_LIMIT, Door::Same, None),
    ],
    exclusive: &[],
};

/// The `refs` operation.
pub static REFS: Op = Op {
    name: "refs",
    summary: "fan-in: what references this symbol",
    tool: Some("find_referencing_symbols"),
    params: &[
        REPO,
        JSON,
        NO_CACHE_CLI_ONLY,
        positional("target", "NAME_OR_STABLE_ID", Arity::One, Door::Same),
        int("limit", &["--limit"], DEFAULT_REFS_LIMIT, Door::Same, None),
        flag(
            "shared_callers",
            &["--shared-callers"],
            Door::Same,
            "instead rank symbols the same callers use (the DRY pass)",
        ),
    ],
    exclusive: &[],
};

/// The `history` operation.
pub static HISTORY: Op = Op {
    name: "history",
    summary: "why does this code exist: the commits that touched a symbol's lines",
    tool: Some("history"),
    params: &[
        REPO,
        JSON,
        NO_CACHE_CLI_ONLY,
        positional("target", "STABLE_ID", Arity::One, Door::Same),
        int(
            "limit",
            &["--limit"],
            DEFAULT_HISTORY_LIMIT,
            Door::Same,
            None,
        ),
        int(
            "budget",
            &["--budget"],
            DEFAULT_HISTORY_BUDGET,
            Door::Same,
            None,
        ),
    ],
    exclusive: &[],
};

/// The `explain` operation.
pub static EXPLAIN: Op = Op {
    name: "explain",
    summary: "one symbol card: definition, fan-out, fan-in",
    tool: Some("explain_symbol"),
    params: &[
        REPO,
        JSON,
        NO_CACHE,
        positional("target", "NAME_OR_STABLE_ID", Arity::One, Door::Same),
        int(
            "limit",
            &["--limit"],
            DEFAULT_EXPLAIN_LIMIT,
            Door::Same,
            Some("rows per section, per tier (default: 20)"),
        ),
    ],
    exclusive: &[],
};

/// The `path` operation.
pub static PATH: Op = Op {
    name: "path",
    summary: "shortest resolved path between two symbols",
    tool: Some("analyze_structure"),
    params: &[
        REPO,
        JSON,
        NO_CACHE,
        positional("source", "FROM", Arity::One, Door::Same),
        positional("target", "TO", Arity::One, Door::Same),
        flag(
            "include_unique",
            &["--include-unique"],
            Door::Same,
            "also walk repository-wide unique-name edges; off by default because \
                 uniqueness is retrieval evidence, not binding evidence",
        ),
        flag(
            "include_ambiguous",
            &["--include-ambiguous"],
            Door::Same,
            "also walk name-only-ambiguous edges; off by default because a path built on a \
                 guessed binding reads like a finding and is not one",
        ),
        int(
            "max_visited",
            &["--max-visited"],
            DEFAULT_MAX_VISITED,
            Door::CliOnly,
            Some(
                "node bound on the search (default: 20000); hitting it is reported, never \
                     silently answered as 'no path'",
            ),
        ),
    ],
    exclusive: &[],
};

/// The `cycles` operation.
pub static CYCLES: Op = Op {
    name: "cycles",
    summary: "module-level import cycles",
    tool: Some("analyze_structure"),
    params: &[
        REPO,
        JSON,
        NO_CACHE,
        int(
            "limit",
            &["--limit"],
            DEFAULT_CYCLE_LIMIT,
            Door::Same,
            Some("cycles listed (default: 20); the count is always complete"),
        ),
    ],
    exclusive: &[],
};

/// The `communities` operation.
pub static COMMUNITIES: Op = Op {
    name: "communities",
    summary: "which files belong together",
    tool: Some("analyze_structure"),
    params: &[
        REPO,
        JSON,
        NO_CACHE,
        float(
            "resolution",
            &["--resolution"],
            "modularity resolution (default: 1); lower groups more coarsely, higher splits \
                 into more, smaller groups",
        ),
        int(
            "limit",
            &["--limit"],
            DEFAULT_COMMUNITY_LIMIT,
            Door::Same,
            Some("communities listed (default: 20); the count is always complete"),
        ),
        int(
            "members",
            &["--members"],
            DEFAULT_MEMBER_LIMIT,
            Door::CliOnly,
            Some("member files listed per community (default: 12)"),
        ),
    ],
    exclusive: &[],
};

/// The `diagram` operation.
pub static DIAGRAM: Op = Op {
    name: "diagram",
    summary: "mermaid flowchart of the module graph",
    tool: Some("analyze_structure"),
    params: &[
        REPO,
        JSON,
        NO_CACHE,
        text(
            "focus",
            &["--focus"],
            Some("FILE_OR_SYMBOL"),
            Door::Same,
            Some("draw only this module's neighbourhood, resolved like map --focus"),
        ),
        int(
            "max_nodes",
            &["--max-nodes"],
            DEFAULT_DIAGRAM_NODES,
            Door::Same,
            Some(
                "modules drawn (default: 40); the rest are announced on an explicit \
                     elision node",
            ),
        ),
        int(
            "max_edges",
            &["--max-edges"],
            DEFAULT_DIAGRAM_EDGES,
            Door::Same,
            Some(
                "arrows drawn (default: 40); reference edges past this bound are announced \
                     in a comment rather than drawn",
            ),
        ),
        flag(
            "communities",
            &["--communities"],
            Door::Named("group_by_communities"),
            "group the modules into community subgraphs",
        ),
        float(
            "resolution",
            &["--resolution"],
            "modularity resolution used by --communities",
        ),
        text(
            "check",
            &["--check"],
            Some("FILE"),
            Door::CliOnly,
            Some(
                "regenerate and compare against FILE instead of printing; exit 0 when \
                     identical, 1 when it has drifted. A leading ```mermaid fence is stripped \
                     before comparing, so a committed .md diagram can be checked as it stands",
            ),
        ),
    ],
    exclusive: &[],
};

/// The `health` operation.
pub static HEALTH: Op = Op {
    name: "health",
    summary: "orphan candidates, unused exports, hubs and complexity hotspots",
    tool: Some("analyze_structure"),
    params: &[
        REPO,
        JSON,
        NO_CACHE,
        int(
            "limit",
            &["--limit"],
            DEFAULT_HEALTH_LIMIT,
            Door::Same,
            Some(
                "rows listed per section (default: 20); every section's count is always \
                     complete",
            ),
        ),
    ],
    exclusive: &[],
};

/// The `html` operation.
pub static HTML: Op = Op {
    name: "html",
    summary: "interactive HTML export of the module graph",
    tool: None,
    params: &[
        REPO,
        NO_CACHE_CLI_ONLY,
        int(
            "max_nodes",
            &["--max-nodes"],
            DEFAULT_HTML_NODES,
            Door::CliOnly,
            Some("modules included (default: 200, maximum: 1000)"),
        ),
        int(
            "max_edges",
            &["--max-edges"],
            DEFAULT_HTML_EDGES,
            Door::CliOnly,
            Some("edges included (default: 600, maximum: 5000)"),
        ),
        Param {
            door: Door::CliOnly,
            ..float(
                "resolution",
                &["--resolution"],
                "modularity resolution used for community colours",
            )
        },
        text(
            "cache_file",
            &["--cache-file"],
            Some("NAME.html"),
            Door::CliOnly,
            Some("write under this repository's XDG cache entry instead of stdout"),
        ),
    ],
    exclusive: &[],
};

/// The `resolve-locs` operation.
pub static RESOLVE_LOCS: Op = Op {
    name: "resolve-locs",
    summary: "turn class:/function:/line: strings into intervals",
    tool: Some("resolve_locations"),
    params: &[
        REPO,
        JSON,
        NO_CACHE_CLI_ONLY,
        positional("file", "FILE", Arity::One, Door::Named("path")),
        Param {
            dest: "loc",
            cli: Some(Cli::Option {
                flags: &["--loc"],
                metavar: Some("LOC"),
                repeat: true,
                required: true,
            }),
            door: Door::Named("locs"),
            value: Value::Text,
            help: None,
        },
        int(
            "context",
            &["--context"],
            DEFAULT_CONTEXT_LINES,
            Door::Named("context_lines"),
            None,
        ),
    ],
    exclusive: &[],
};

/// The `warmup` operation.
pub static WARMUP: Op = Op {
    name: "warmup",
    summary: "fetch and probe grammars; fails loudly",
    tool: None,
    params: &[
        positional("languages", "LANGUAGE", Arity::Any, Door::CliOnly),
        flag(
            "no_download",
            &["--no-download"],
            Door::CliOnly,
            "air-gap mode: a grammar that would need fetching is an error",
        ),
    ],
    exclusive: &[],
};

/// The `index` operation.
pub static INDEX: Op = Op {
    name: "index",
    summary: "build or refresh the tag cache",
    tool: None,
    params: &[
        REPO,
        JSON,
        flag(
            "force",
            &["--force"],
            Door::CliOnly,
            "re-extract every file instead of reusing rows whose sha256 still matches",
        ),
    ],
    exclusive: &[],
};

/// The `guide` operation.
pub static GUIDE: Op = Op {
    name: "guide",
    summary: "print the packaged agent usage guide",
    tool: None,
    params: &[text(
        "section",
        &["--section"],
        Some("NAME"),
        Door::CliOnly,
        Some(
            "print one section: a tool name ('refs', 'map', 'communities') or the heading \
                 lowercased and hyphenated. An unknown name lists them all",
        ),
    )],
    exclusive: &[],
};

/// Every read operation, in the order the command line lists its subcommands.
pub static OPS: [&Op; 19] = [
    &MAP,
    &TREE,
    &SKELETON,
    &EXPAND,
    &SLICE,
    &FIND_SYMBOL,
    &REFS,
    &HISTORY,
    &EXPLAIN,
    &PATH,
    &CYCLES,
    &COMMUNITIES,
    &DIAGRAM,
    &HEALTH,
    &HTML,
    &RESOLVE_LOCS,
    &WARMUP,
    &INDEX,
    &GUIDE,
];
