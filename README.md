# agentless-mcp

`agentless-mcp` provides tree-sitter-based code navigation, repository
structure analysis, and patch validation for local repositories. It exposes
the same functionality through a command-line interface and a read-only
stdio MCP server.

## Install

```sh
uv tool install agentless-mcp
```

Install the MCP server support when needed:

```sh
uv tool install "agentless-mcp[mcp]"
```

Before analyzing a language for the first time, download and verify its
grammar:

```sh
agentless-mcp warmup
```

## CLI

Most commands analyze the repository containing the current directory. Use
`--repo PATH` to select another repository. Add `--json` where supported for
machine-readable output.

### Navigate code

```sh
agentless-mcp map --focus src/app.py
agentless-mcp tree --depth 3
agentless-mcp skeleton src/app.py
agentless-mcp expand py:src/app.py::App.run
agentless-mcp slice src/app.py --lines 40:80
agentless-mcp find-symbol App
agentless-mcp refs App.run
agentless-mcp explain App.run
```

These commands provide repository maps, directory trees, symbol overviews,
full symbol bodies, source slices, symbol lookup, references, and symbol
context. Symbol IDs are printed by `map` and `skeleton` and can be passed to
`expand`, `refs`, `explain`, and related commands.

### Analyze structure

```sh
agentless-mcp path App.run Database.connect
agentless-mcp cycles
agentless-mcp communities
agentless-mcp diagram > modules.mmd
agentless-mcp html > modules.html
```

These commands find relationships between symbols or files, report import
cycles, group related files, and export Mermaid or interactive HTML graphs.

### Validate patches

The CLI also supports deterministic patch workflows:

```sh
agentless-mcp patch parse --file change.patch
agentless-mcp patch check --file change.patch --repo /path/to/repo
agentless-mcp patch apply --file change.patch --repo /path/to/repo
agentless-mcp lint --candidates ./candidates --repo /path/to/repo
agentless-mcp validate --candidates ./candidates --repo /path/to/repo \
  --test-cmd 'pytest -q'
agentless-mcp vote --verdicts verdicts.jsonl
```

Patch candidates can use SEARCH/REPLACE text or the package's `edits.json`
format. `validate` runs candidates against the repository tests, and `vote`
ranks the candidates that pass.

### Cache and capabilities

Parsing happens on demand. Build an optional repository cache to improve
repeated queries:

```sh
agentless-mcp index --repo /path/to/repo
agentless-mcp capabilities --repo /path/to/repo
```

Use `--no-cache` on repository-scoped commands to bypass the cache.

## MCP server

The server communicates over stdio and exposes read-only repository tools.
Start it with one or more explicitly allowed repository roots:

```sh
agentless-mcp-server --root /path/to/repo
```

The MCP tools are:

| Tool | Purpose |
| --- | --- |
| `repo_map` | Rank relevant files and symbols within a token budget |
| `list_dir` | List the repository tree |
| `get_symbols_overview` | Show declarations without symbol bodies |
| `expand_symbols` | Return full symbol bodies |
| `read_slice` | Read selected source lines |
| `find_symbol` | Find symbols by name |
| `find_referencing_symbols` | Find references and callers |
| `explain_symbol` | Show a symbol with its relationships |
| `analyze_structure` | Query paths, cycles, communities, or diagrams |
| `resolve_locations` | Resolve class, function, and line locations |
| `capabilities` | Report loaded grammars and cache state |

The MCP server does not apply patches or execute repository commands.

## Supported languages

The bundled grammars support Bash, C, C++, C#, Go, HCL, Java, JavaScript, JSON,
Kotlin, Lua, PHP, Python, Ruby, Rust, Scala, SQL, Swift, TOML, TSX, TypeScript,
and YAML. Run `capabilities` to see the grammars available in the current
installation.

## License

MIT. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
