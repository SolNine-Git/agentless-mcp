"""Identifier-reference pass and the repository scan that feeds every view.

The extractor answers "what does this file define". This module answers the
other half -- "where is that name used" -- by walking each parsed tree and
collecting every identifier-class leaf. A reference is deliberately a
``(file, name, line)`` triple and nothing more: resolving a name to the
declaration it really binds to needs type information this tool does not have
and will not pretend to have.

Fan-in is therefore fuzzy by construction: the references to a symbol are the
sites that spell its name outside its own definition span. That over-reports
across unrelated files that happen to share a short name -- which is exactly
what the log damping in :mod:`agentless_mcp.core.graph` and the stoplist knob
there exist to absorb -- and it never silently under-reports, which is the
failure mode that matters for a blast-radius question.

:func:`scan_repo` is the one traversal every service shares: walk the
repository once, parse each supported file once, and hand back symbols,
imports and references together. Files that cannot be read, are too large, or
whose grammar is not warmed are reported in ``skipped`` with the reason --
never dropped into an answer that then looks complete.
"""

from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path

from tree_sitter import Node

from agentless_mcp.core import grammars
from agentless_mcp.core.extractor import LANGUAGE_CONFIGS, TreeSitterExtractor
from agentless_mcp.core.imports import ImportStatement
from agentless_mcp.core.symbols import ASTSymbol
from agentless_mcp.core.treewalk import walk_repo
from agentless_mcp.util.errors import LanguageUnavailable
from agentless_mcp.util.fslimits import DEFAULT_MAX_FILE_BYTES, read_bounded

# Identifier node types for the languages whose extraction is done by a
# dedicated handler, so they have no LanguageConfig entry to carry them. Same
# split as `skeleton._EXTRA_FUNCTION_NODE_TYPES`, for the same reason.
_EXTRA_IDENTIFIER_NODE_TYPES: dict[str, tuple[str, ...]] = {
    "python": ("identifier",),
    "rust": ("identifier", "type_identifier", "field_identifier"),
    "c": ("identifier", "type_identifier", "field_identifier"),
    "cpp": ("identifier", "type_identifier", "field_identifier", "namespace_identifier"),
}


def identifier_node_types(language: str) -> frozenset[str]:
    """Return the leaf node types that name something in ``language``."""
    config = LANGUAGE_CONFIGS.get(language)
    if config is not None:
        return frozenset(config.identifier_node_types)
    return frozenset(_EXTRA_IDENTIFIER_NODE_TYPES.get(language, ("identifier",)))


@dataclass(frozen=True)
class Ref:
    """One identifier occurrence: which file spelled which name, and where."""

    path: str
    name: str
    line: int


@dataclass(frozen=True)
class FileFacts:
    """Everything one parsed file contributes to the read surface."""

    path: str
    language: str
    line_count: int
    symbols: tuple[ASTSymbol, ...]
    imports: tuple[ImportStatement, ...]
    refs: tuple[Ref, ...]


@dataclass(frozen=True)
class SkippedFile:
    """A file the scan saw but did not parse, and why."""

    path: str
    reason: str


@dataclass(frozen=True)
class RepoScan:
    """One traversal of a repository: what parsed, and what did not."""

    root: Path
    files: tuple[FileFacts, ...]
    skipped: tuple[SkippedFile, ...]

    def by_path(self) -> dict[str, FileFacts]:
        """Index the parsed files by their repository-relative path."""
        return {facts.path: facts for facts in self.files}


@dataclass(frozen=True)
class Definition:
    """A symbol together with the file that defines it."""

    path: str
    symbol: ASTSymbol


@dataclass(frozen=True)
class RefIndex:
    """Name-keyed definitions and reference sites across a whole scan."""

    definitions: Mapping[str, tuple[Definition, ...]]
    sites: Mapping[str, tuple[Ref, ...]]
    files_referencing: Mapping[str, int]

    def defining_paths(self, name: str) -> tuple[str, ...]:
        """The distinct files defining ``name``, in path order."""
        return tuple(sorted({definition.path for definition in self.definitions.get(name, ())}))


def collect_refs(source: str, language: str, path: str) -> list[Ref]:
    """Return every identifier occurrence in ``source``.

    Raises :class:`LanguageUnavailable` when the grammar is not warmed: a
    reference pass that quietly returns nothing would read as "this symbol is
    unused", which is the most expensive wrong answer this tool could give.
    """
    wanted = identifier_node_types(language)
    if not wanted:
        return []

    parser = grammars.get_parser(language)
    data = source.encode("utf-8")
    tree = parser.parse(data)

    refs: list[Ref] = []
    for node in _walk(tree.root_node):
        if node.type not in wanted or node.child_count:
            continue
        name = data[node.start_byte : node.end_byte].decode("utf-8", errors="replace")
        if name:
            refs.append(Ref(path=path, name=name, line=node.start_point[0] + 1))
    return refs


def scan_repo(
    root: Path,
    extractor: TreeSitterExtractor,
    *,
    max_file_bytes: int = DEFAULT_MAX_FILE_BYTES,
) -> RepoScan:
    """Walk ``root`` once and parse every supported file it holds."""
    files: list[FileFacts] = []
    skipped: list[SkippedFile] = []

    for repo_file in walk_repo(root):
        language = TreeSitterExtractor.SUPPORTED_EXTENSIONS.get(Path(repo_file.path).suffix)
        if language is None:
            continue

        read = read_bounded(root / repo_file.path, max_bytes=max_file_bytes)
        if read.text is None:
            skipped.append(SkippedFile(path=repo_file.path, reason=read.skipped or "unreadable"))
            continue

        facts = _parse_one(read.text, language, repo_file.path, extractor)
        if isinstance(facts, SkippedFile):
            skipped.append(facts)
        else:
            files.append(facts)

    return RepoScan(root=root, files=tuple(files), skipped=tuple(skipped))


def build_ref_index(scan: RepoScan) -> RefIndex:
    """Index a scan by name: where each name is defined and where it is used."""
    definitions: dict[str, list[Definition]] = {}
    sites: dict[str, list[Ref]] = {}
    referencing: dict[str, set[str]] = {}

    for facts in scan.files:
        for symbol in facts.symbols:
            definitions.setdefault(symbol.name, []).append(
                Definition(path=facts.path, symbol=symbol)
            )
        for ref in facts.refs:
            sites.setdefault(ref.name, []).append(ref)
            referencing.setdefault(ref.name, set()).add(ref.path)

    return RefIndex(
        definitions={name: tuple(values) for name, values in definitions.items()},
        sites={name: tuple(values) for name, values in sites.items()},
        files_referencing={name: len(paths) for name, paths in referencing.items()},
    )


def references_to(index: RefIndex, definition: Definition) -> tuple[Ref, ...]:
    """Return the sites that spell a definition's name from outside its body.

    The definition's own span is excluded, which also removes the declaration
    line itself: the identifier in ``class Invoice`` is a reference site like
    any other, and counting it would make every symbol look used once.
    """
    symbol = definition.symbol
    start = symbol.line_number
    end = symbol.end_line_number if symbol.end_line_number is not None else symbol.line_number

    return tuple(
        ref
        for ref in index.sites.get(symbol.name, ())
        if not (ref.path == definition.path and start <= ref.line <= end)
    )


def enclosing_symbol(facts: FileFacts, line: int) -> ASTSymbol | None:
    """Return the innermost symbol whose span contains ``line``.

    Innermost by start line: a method inside a class contains fewer lines and
    starts later, so the deepest containing span is always the one that starts
    last. Attribution for a reference site is that symbol's qualified name.
    """
    containing = [
        symbol
        for symbol in facts.symbols
        if symbol.line_number <= line <= (symbol.end_line_number or symbol.line_number)
    ]
    if not containing:
        return None
    return max(containing, key=lambda symbol: symbol.line_number)


def symbols_by_qualname(facts: FileFacts) -> dict[str, ASTSymbol]:
    """Index one file's symbols by qualified name, first definition winning."""
    indexed: dict[str, ASTSymbol] = {}
    for symbol in facts.symbols:
        key = f"{symbol.parent_class}.{symbol.name}" if symbol.parent_class else symbol.name
        indexed.setdefault(key, symbol)
    return indexed


def _parse_one(
    text: str,
    language: str,
    path: str,
    extractor: TreeSitterExtractor,
) -> FileFacts | SkippedFile:
    """Parse one file three ways, degrading that file alone when it cannot be."""
    try:
        symbols = extractor.extract_from_source(text, language, path)
        imports = extractor.extract_imports_from_source(text, language, path)
        refs = collect_refs(text, language, path)
    except LanguageUnavailable as exc:
        return SkippedFile(path=path, reason=str(exc))

    return FileFacts(
        path=path,
        language=language,
        line_count=len(text.split("\n")),
        symbols=tuple(symbols),
        imports=tuple(imports),
        refs=tuple(refs),
    )


def _walk(root: Node) -> list[Node]:
    """Return every node in the tree, parents before children.

    Iterative rather than recursive: a deeply nested expression in a generated
    file must not turn a repository map into a RecursionError.
    """
    found: list[Node] = []
    stack: list[Node] = [root]
    while stack:
        node = stack.pop()
        found.append(node)
        stack.extend(reversed(node.children))
    return found
