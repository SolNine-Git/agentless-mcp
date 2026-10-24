//! The tool descriptions.
//! `cargo xtask generate` writes this file from the prompt JSON files.

/// The description of the `analyze_structure` tool.
pub const ANALYZE_STRUCTURE: &str = "Five whole-repository structure questions behind one \
        operation argument: how are these two symbols connected (path), \
        where are the import knots (cycles), which files belong \
        together (communities), what does the module graph look like \
        (diagram), what is unreferenced, central or complex (health). \
        operation='path' takes source and target (a stable id, a \
        qualified name or a file path) and returns the fewest-hop chain \
        of resolved relationships between them, each hop carrying its \
        relation, evidence tier and file:line -- ask it when the \
        question is how a change here could reach a failure there; only \
        same-file and resolved-via-import edges participate by default. \
        Repository-wide unique-name edges require include_unique and \
        name-only-ambiguous edges require include_ambiguous, so the \
        default path never rests on name-only binding evidence. \
        operation='cycles' takes nothing and returns every module-level \
        import cycle as a file -> file -> file chain that closes -- ask \
        it when an import error, a partially initialized module or a \
        layering question needs the knots named. \
        operation='communities' takes an optional resolution (lower \
        groups more coarsely) and returns which files belong together, \
        each group labelled by the directory prefix most of its members \
        share; modularity below 0.3 is marked as a weak partition to \
        use only as a hint -- ask it to orient in an unfamiliar \
        repository before reading any file. operation='diagram' takes \
        optional focus, max_nodes and group_by_communities and returns \
        a mermaid flowchart of the module graph, PageRank-bounded with \
        an explicit elision marker; solid arrows are declared imports, \
        dashed arrows are discounted name references, drawn only while \
        the diagram stays legible and counted in a comment when left \
        out -- ask for it only when a human will look at it, because \
        the facts you should reason over are in the text views. \
        operation='health' takes an optional limit and returns three \
        readings of one degree count -- orphan candidates (a function \
        no counted edge reaches or leaves), unused exports (a public \
        function no counted edge reaches) and hubs (the highest total \
        degree first); only same-file and resolved-via-import edges are \
        counted, and a unique-name or name-only-ambiguous match is \
        named on the row rather than counted, so an orphan row states \
        the evidence it was not built on. The health answer also \
        carries a complexity summary (functions measured, functions \
        above the complexity threshold, which is 10 by default, \
        erosion, the share of mass held by those functions, and clone \
        share, the share of source lines inside a block that has a copy \
        once names and literals are ignored) and hotspots, the \
        functions with the most mass (cc times the square root of \
        sloc), each with its stable id or marked metric-only. \
        Complexity counts decision points and lines. It does not judge \
        design. No path, no cycles and an unknown endpoint are all \
        ordinary answers with a message rather than errors; an unknown \
        operation is rejected by the published enum.";

/// The description of the `capabilities` tool.
pub const CAPABILITIES: &str = "Why is this answer incomplete? The server version, grammars, \
        cache state, roots and caps. Call it when a language seems \
        unsupported or the wrong root was selected.";

/// The description of the `expand_symbols` tool.
pub const EXPAND_SYMBOLS: &str = "You have located the symbols; now read their bodies. The \
        escalation step after a skeleton view: returns the \
        line-numbered body of each named symbol. stable_ids come from \
        repo_map, get_symbols_overview, find_symbol, or \
        resolve_locations output (form: \
        py:src/app/svc.py::Invoice.total). Every requested id comes \
        back: when the batch exceeds the output budget the longest \
        bodies are cut to their leading lines, each marked with how \
        many lines it shows out of how many, and a summary line names \
        how many were shortened. To read a long body whole, expand that \
        id on its own.";

/// The description of the `explain_symbol` tool.
pub const EXPLAIN_SYMBOL: &str = "What is this symbol and how is it wired in? Start here when you \
        have one suspect symbol and have not seen it before. One call \
        returns its definition site and signature, rationale comments \
        linked to it, what it references (fan-out) and what references \
        it (fan-in), each grouped by evidence tier, plus the import \
        relationships of the file it lives in. Prefer it over \
        find_symbol followed by find_referencing_symbols: the same \
        answer in one round trip instead of two or three. Rows resolved \
        only by name list every candidate rather than picking one; \
        target is a stable id or a qualified name.";

/// The description of the `find_referencing_symbols` tool.
pub const FIND_REFERENCING_SYMBOLS: &str = "Who calls this symbol, and what breaks if it changes? \
        Lists the symbols that reference a target, by file and evidence \
        tier: same-file, resolved-via-import, unique, \
        name-only-ambiguous. Read the top two tiers as callers and the \
        bottom two as candidates. Call it when the answer depends on \
        the callers. It is the most expensive tool here: fan-in over a \
        large repository can take minutes.";

/// The description of the `find_symbol` tool.
pub const FIND_SYMBOL: &str = "Does this symbol exist, and where is it defined? Find symbols by \
        substring or qualified name, optionally filtered by kind. Each \
        hit carries file:line and a stable id usable with \
        expand_symbols.";

/// The description of the `get_symbols_overview` tool.
pub const GET_SYMBOLS_OVERVIEW: &str = "What does this file declare? Render the named files as \
        signatures with bodies elided -- the cheap first look, instead \
        of reading the file. Each file's block opens with a stable ids: \
        line naming the id pattern for that file (nested symbols \
        qualify as Class.method), so an id for expand_symbols reads \
        straight off the overview. Escalate to expand_symbols for the \
        few symbols you need rather than re-reading whole files.";

/// The description of the `history` tool.
pub const HISTORY: &str = "Why does this code exist? The commits that changed one symbol's lines, \
        newest first, with full message bodies. Call it after another \
        tool tells you where.";

/// The description of the `list_dir` tool.
pub const LIST_DIR: &str = "What exists in this repository? List the files as a tree, honouring \
        gitignore. Pass an optional repository-relative path to render \
        only that directory's subtree without hiding other tracked \
        paths by policy. For orientation when you need paths rather \
        than symbols; depth and max_entries are capped.";

/// The description of the `orient` tool.
pub const ORIENT: &str = "Rank the files and symbols most relevant to a focus, or show how the \
        repository is built. map (focus, limit, granularity, budget): a \
        token-budgeted map of the highest-ranked files. Call it when \
        the location is unknown or the change spans files. Search text \
        with Grep: the map ranks symbols, so a file that matches only \
        by a string does not appear. budget counts chars/4: a real BPE \
        count runs 10-35% higher. communities (resolution, limit): \
        which files belong together. cycles (limit): module import \
        cycles. diagram (focus, max_nodes, max_edges, \
        group_by_communities, resolution): the module graph as mermaid, \
        for a human. path (requires source and target; include_unique, \
        include_ambiguous): the fewest-hop resolved chain between two \
        endpoints. health (limit): orphan candidates, unused exports, \
        hubs and complexity hotspots.";

/// The description of the `read` tool.
pub const READ: &str = "Read numbered lines of one file with enclosing-scope headers (slice), or \
        list the repository tree with gitignore applied (dir). slice \
        (requires path; lines or whole_file, not both; context_lines). \
        dir (path, default the root; depth, max_entries). For one whole \
        symbol, symbols expand fits better.";

/// The description of the `read_slice` tool.
pub const READ_SLICE: &str = "You need exact lines and no symbol boundary fits: return numbered \
        lines for explicit 1-based inclusive [start, end] ranges in one \
        file, with enclosing-scope headers. The MCP call requires \
        non-empty lines unless whole_file=true is explicitly set. A \
        range starting beyond the file is reported with the file's true \
        line count, never answered with substitute content; a range \
        running past the end keeps its clamped tail. Line-level is the \
        last resort; prefer expand_symbols when a symbol boundary fits.";

/// The description of the `repo_map` tool.
pub const REPO_MAP: &str = "Where does this bug or feature live? The structural entry point: pass \
        focus to seed the ranking toward the ask, and a token-budgeted \
        skeleton of the most relevant files comes back. It is a top-N \
        view rather than an enumeration: max_files defaults to 10, so \
        ten files come back however large the repository is, and an \
        auto-sized budget lands in a 2000-8000 token band, counted by \
        the server's own chars/4 estimator and not by your model's \
        tokenizer -- on this tool's id-dense output a real BPE count \
        runs 10-35% higher, the band crates/app/src/tokens/tests.rs \
        pins, so read budget as a reproducible knob rather than a \
        promise about your bill. To orient in a repository you have not \
        read yet, call analyze_structure(operation='communities') first \
        for the file groups, then map into the group that matters. \
        Routing rule: use grep when the literal string or file is \
        already known; use this map when the target location is unknown \
        or the change surface spans files. NOTE, WHY, HACK and TODO \
        comments plus ADR/RFC citations appear below their enclosing \
        symbol as linked rationale nodes. It ranks symbols, so it will \
        not surface a file that matches your issue only as a string -- \
        test fixtures, config and resource files, and modules reached \
        by a setting name rather than a call edge. Pair it with a text \
        search over the issue's distinctive literals rather than \
        treating either pass as complete. A focus entry is a file path, \
        a path suffix, a bare module stem ('envelope' for envelope.py), \
        a qualified symbol name, or a bare function, method or class \
        name matched exactly against the extracted symbols; each entry \
        carries one vote split across the files it matched. A traceback \
        frame, a blob URL, an absolute path, or a path:line spelling \
        seeds the file it names -- paste it as it appears in the task. \
        Entries that match nothing are listed in unresolved_seeds and \
        noted above the map -- if you see them, the ranking is not \
        focused the way you asked. budget is in tokens (auto-sized by \
        default); granularity is 'function' (default), 'file', or \
        'body' -- 'body' returns the ranked file rows plus budgeted \
        bodies in one answer, shortened when necessary, replacing a map \
        followed by an expand -- a symbol the focus named is expanded \
        first, and the top-scored symbols fill the remaining seats. \
        Omitted arguments fall back to the repository's \
        .agentless-mcp.json, then to the built-in defaults named above. \
        A ranked file's header may carry churn -- '4c/90d, last 3d' is \
        commits in the last 90 days and days since the newest -- when \
        git answers for the tree; it is evidence to weigh, and it never \
        moves the ranking. At granularity 'function' each file's block \
        opens with a stable ids: line naming that file's id pattern, \
        and every symbol row below carries the qualified name alone; \
        join the two to build an id expand accepts. A file with no row \
        under it prints no pattern, and granularity 'file' prints none \
        at all. A position is an @line suffix, never an N| gutter -- \
        the gutter means verbatim file content and a map row is a \
        normalized signature.";

/// The description of the `resolve_locations` tool.
pub const RESOLVE_LOCATIONS: &str = "Which lines hold this class, function, or location? Turn \
        location strings (class:Invoice, function:Invoice.total, \
        line:42) into merged line intervals and stable ids for one \
        file. Unrecognized locations are reported individually, never \
        dropped silently.";

/// The description of the `symbols` tool.
pub const SYMBOLS: &str = "Look up a symbol and read it by symbol. find (requires name; kind, \
        limit): symbols matching a name, Qualified.name or substring, \
        with path:line and a stable id. overview (requires paths; \
        docstrings): signatures with bodies elided; read it instead of \
        the file. expand (requires stable_ids; limit): line-numbered \
        bodies; expand a long id alone, because an over-budget batch is \
        shortened. explain (requires target; limit): one symbol's \
        definition, signature, fan-out and fan-in by evidence tier, and \
        imports. locate (requires path and locations; context_lines): \
        class:, function: and line: selectors as stable ids and line \
        intervals. For callers and blast radius, use \
        find_referencing_symbols.";
